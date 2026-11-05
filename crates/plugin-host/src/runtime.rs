use std::io::{self, Stdin, Stdout};
use std::process::ExitCode;

use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap, TypedFunc,
};

use crate::{Reply, Request, Start, read_frame, write_frame};

const MAX_ANSWER_BYTES: usize = 1 << 20;

const ERR_FAILED: i64 = -2;
const ERR_NO_ROOM: i64 = -3;

struct Host {
    limits: StoreLimits,
    pipe_in: Stdin,
    pipe_out: Stdout,
}

impl Host {
    fn ask(&mut self, request: &Request, bytes: &[u8]) -> wasmtime::Result<Option<Vec<u8>>> {
        write_frame(&mut self.pipe_out.lock(), request, bytes)?;
        let (reply, bytes): (Reply, _) = read_frame(&mut self.pipe_in.lock())?;
        Ok(reply.ok.then_some(bytes))
    }
}

fn unpack(packed: i64) -> (usize, usize) {
    let bits = packed.cast_unsigned();
    ((bits >> 32) as usize, (bits & 0xffff_ffff) as usize)
}

fn pack(at: i32, len: usize) -> i64 {
    (i64::from(at.cast_unsigned()) << 32) | (len as i64)
}

fn guest_memory(caller: &mut Caller<'_, Host>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => Some(memory),
        _ => None,
    }
}

fn guest_bytes(caller: &Caller<'_, Host>, memory: Memory, at: i32, len: i32) -> Option<Vec<u8>> {
    let (start, len) = (at.cast_unsigned() as usize, len.cast_unsigned() as usize);
    memory
        .data(caller)
        .get(start..start.saturating_add(len))
        .map(<[u8]>::to_vec)
}

fn guest_name(caller: &Caller<'_, Host>, memory: Memory, at: i32, len: i32) -> Option<String> {
    String::from_utf8(guest_bytes(caller, memory, at, len)?).ok()
}

fn give(caller: &mut Caller<'_, Host>, memory: Memory, bytes: &[u8]) -> wasmtime::Result<i64> {
    let Some(Extern::Func(alloc)) = caller.get_export("alloc") else {
        return Ok(ERR_NO_ROOM);
    };
    let alloc = alloc.typed::<i32, i32>(&*caller)?;
    let Ok(size) = i32::try_from(bytes.len()) else {
        return Ok(ERR_NO_ROOM);
    };
    let place = alloc.call(&mut *caller, size)?;
    let written = memory.write(&mut *caller, place.cast_unsigned() as usize, bytes);
    Ok(if written.is_ok() {
        pack(place, bytes.len())
    } else {
        ERR_NO_ROOM
    })
}

fn read_file(mut caller: Caller<'_, Host>, at: i32, len: i32) -> wasmtime::Result<i64> {
    let Some(memory) = guest_memory(&mut caller) else {
        return Ok(ERR_FAILED);
    };
    let Some(name) = guest_name(&caller, memory, at, len) else {
        return Ok(ERR_FAILED);
    };
    match caller.data_mut().ask(&Request::ReadFile { name }, &[])? {
        Some(contents) => give(&mut caller, memory, &contents),
        None => Ok(ERR_FAILED),
    }
}

fn write_file(
    mut caller: Caller<'_, Host>,
    path_at: i32,
    path_len: i32,
    data_at: i32,
    data_len: i32,
) -> wasmtime::Result<i64> {
    let Some(memory) = guest_memory(&mut caller) else {
        return Ok(ERR_FAILED);
    };
    let name = guest_name(&caller, memory, path_at, path_len);
    let data = guest_bytes(&caller, memory, data_at, data_len);
    let (Some(name), Some(data)) = (name, data) else {
        return Ok(ERR_FAILED);
    };
    let written = caller.data_mut().ask(&Request::WriteFile { name }, &data)?;
    Ok(if written.is_some() { 0 } else { ERR_FAILED })
}

fn run_host(mut caller: Caller<'_, Host>, at: i32, len: i32) -> wasmtime::Result<i64> {
    let Some(memory) = guest_memory(&mut caller) else {
        return Ok(ERR_FAILED);
    };
    let Some(request) = guest_bytes(&caller, memory, at, len) else {
        return Ok(ERR_FAILED);
    };
    match caller.data_mut().ask(&Request::Run, &request)? {
        Some(answer) => give(&mut caller, memory, &answer),
        None => Ok(ERR_FAILED),
    }
}

fn describe(error: &wasmtime::Error) -> String {
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "ran too long and was stopped".to_owned(),
        Some(trap) => format!("crashed: {trap}"),
        None => format!("failed: {error:#}"),
    }
}

fn call(start: &Start, module: &[u8], input: &[u8], host: Host) -> Result<Vec<u8>, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config).map_err(|e| describe(&e))?;
    let module = Module::new(&engine, module).map_err(|e| format!("not a valid module: {e:#}"))?;
    let mut store = Store::new(&engine, host);
    store.limiter(|host| &mut host.limits);
    store.set_fuel(start.fuel).map_err(|e| describe(&e))?;
    let mut linker = Linker::new(&engine);
    linker
        .func_wrap("idea", "read_file", read_file)
        .and_then(|l| l.func_wrap("idea", "write_file", write_file))
        .and_then(|l| l.func_wrap("idea", "run", run_host))
        .map_err(|e| describe(&e))?;
    let instance: Instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| describe(&e))?;
    let memory: Memory = instance
        .get_memory(&mut store, "memory")
        .ok_or("it exports no `memory`")?;
    let alloc: TypedFunc<i32, i32> = instance
        .get_typed_func(&mut store, "alloc")
        .map_err(|e| describe(&e))?;
    let handle: TypedFunc<(i32, i32), i64> = instance
        .get_typed_func(&mut store, "handle")
        .map_err(|e| describe(&e))?;
    let size = i32::try_from(input.len()).map_err(|_| "the input is too large")?;
    let at = alloc.call(&mut store, size).map_err(|e| describe(&e))?;
    memory
        .write(&mut store, at.cast_unsigned() as usize, input)
        .map_err(|_| "it gave no room for the input")?;
    let packed = handle
        .call(&mut store, (at, size))
        .map_err(|e| describe(&e))?;
    let (from, len) = unpack(packed);
    if len > MAX_ANSWER_BYTES {
        return Err("its answer is too large".to_owned());
    }
    let answer = memory
        .data(&store)
        .get(from..from.saturating_add(len))
        .ok_or("its answer points outside its memory")?
        .to_vec();
    Ok(answer)
}

fn serve_once() -> io::Result<()> {
    let (pipe_in, pipe_out) = (io::stdin(), io::stdout());
    let (start, bytes): (Start, Vec<u8>) = read_frame(&mut pipe_in.lock())?;
    if start.input_len > bytes.len() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let (input, module) = bytes.split_at(start.input_len);
    let host = Host {
        limits: StoreLimitsBuilder::new()
            .memory_size(start.memory_bytes)
            .instances(1)
            .tables(4)
            .memories(1)
            .build(),
        pipe_in,
        pipe_out: io::stdout(),
    };
    let (error, answer) = match call(&start, module, input, host) {
        Ok(answer) => (None, answer),
        Err(error) => (Some(error), Vec::new()),
    };
    write_frame(&mut pipe_out.lock(), &Request::Done { error }, &answer)
}

pub fn serve() -> ExitCode {
    match serve_once() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
