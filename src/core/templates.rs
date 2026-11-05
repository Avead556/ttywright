pub struct FileTemplate {
    pub label: &'static str,
    pub file: &'static str,
    pub contents: &'static str,
    pub executable: bool,
}

pub struct ProjectTemplate {
    pub label: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub const FILE_TEMPLATES: &[FileTemplate] = &[
    FileTemplate {
        label: r####".gitignore: General"####,
        file: r####".gitignore"####,
        contents: r####".DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Rust"####,
        file: r####".gitignore"####,
        contents: r####"/target
**/*.rs.bk
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: PHP"####,
        file: r####".gitignore"####,
        contents: r####"/vendor/
.phpunit.result.cache
.phpunit.cache/
.php-cs-fixer.cache
composer.phar
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Laravel"####,
        file: r####".gitignore"####,
        contents: r####"/vendor/
/node_modules/
/public/build
/public/hot
/public/storage
/storage/*.key
.phpunit.result.cache
Homestead.json
Homestead.yaml
npm-debug.log
yarn-error.log
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Symfony"####,
        file: r####".gitignore"####,
        contents: r####"/vendor/
/var/
/public/bundles/
/.env.local.php
/.env.*.local
/config/secrets/prod/prod.decrypt.private.php
.phpunit.result.cache
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Node"####,
        file: r####".gitignore"####,
        contents: r####"node_modules/
dist/
build/
coverage/
*.log
.npm/
.eslintcache
*.tsbuildinfo
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Python"####,
        file: r####".gitignore"####,
        contents: r####"__pycache__/
*.py[cod]
.venv/
venv/
dist/
build/
*.egg-info/
.pytest_cache/
.mypy_cache/
.ruff_cache/
.coverage
htmlcov/
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Go"####,
        file: r####".gitignore"####,
        contents: r####"/bin/
*.exe
*.test
*.out
coverage.txt
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".gitignore: Java"####,
        file: r####".gitignore"####,
        contents: r####"target/
build/
.gradle/
*.class
*.jar
*.war
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".dockerignore"####,
        file: r####".dockerignore"####,
        contents: r####".git
.gitignore
.idea
.vscode
node_modules
vendor
target
__pycache__
.env
*.log
Dockerfile
docker-compose.yml
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: PHP-FPM"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM php:8.3-fpm-alpine

RUN apk add --no-cache git unzip \
    && docker-php-ext-install pdo_mysql opcache

COPY --from=composer:2 /usr/bin/composer /usr/bin/composer

WORKDIR /var/www/html
COPY composer.json composer.lock* ./
RUN composer install --no-dev --no-scripts --prefer-dist --no-interaction
COPY . .
RUN composer dump-autoload --optimize --no-dev

USER www-data
EXPOSE 9000
CMD ["php-fpm"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: PHP with Apache"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM php:8.3-apache

RUN a2enmod rewrite && docker-php-ext-install pdo_mysql opcache

COPY . /var/www/html/
RUN chown -R www-data:www-data /var/www/html

EXPOSE 80
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: Node"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM node:22-alpine AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build --if-present

FROM node:22-alpine
WORKDIR /app
ENV NODE_ENV=production
COPY package*.json ./
RUN npm ci --omit=dev
COPY --from=build /app .
USER node
EXPOSE 3000
CMD ["npm", "start"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: Python"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM python:3.12-slim
WORKDIR /app
ENV PYTHONUNBUFFERED=1
COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
RUN useradd --create-home app
USER app
CMD ["python", "main.py"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: Go"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM golang:1.23-alpine AS build
WORKDIR /src
COPY go.mod go.sum* ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /app .

FROM alpine:3.20
COPY --from=build /app /app
USER nobody
ENTRYPOINT ["/app"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: Rust"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
# Replace `app` with the binary's name from Cargo.toml.
COPY --from=build /src/target/release/app /usr/local/bin/app
USER nobody
ENTRYPOINT ["app"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Dockerfile: Java (Maven)"####,
        file: r####"Dockerfile"####,
        contents: r####"FROM maven:3-eclipse-temurin-21 AS build
WORKDIR /src
COPY pom.xml .
RUN mvn -q dependency:go-offline
COPY src src
RUN mvn -q package -DskipTests

FROM eclipse-temurin:21-jre
COPY --from=build /src/target/*.jar /app.jar
USER nobody
ENTRYPOINT ["java", "-jar", "/app.jar"]
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"docker-compose: PHP, nginx, MySQL"####,
        file: r####"docker-compose.yml"####,
        contents: r####"services:
  app:
    build: .
    volumes:
      - .:/var/www/html
    depends_on:
      - db

  nginx:
    image: nginx:1.27-alpine
    ports:
      - "8080:80"
    volumes:
      - .:/var/www/html:ro
      - ./nginx.conf:/etc/nginx/conf.d/default.conf:ro
    depends_on:
      - app

  db:
    image: mysql:8.4
    environment:
      MYSQL_DATABASE: ${DB_DATABASE:-app}
      MYSQL_USER: ${DB_USERNAME:-app}
      MYSQL_PASSWORD: ${DB_PASSWORD:?set DB_PASSWORD in .env}
      MYSQL_ROOT_PASSWORD: ${DB_ROOT_PASSWORD:?set DB_ROOT_PASSWORD in .env}
    ports:
      - "127.0.0.1:3306:3306"
    volumes:
      - db:/var/lib/mysql

volumes:
  db:
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"docker-compose: Node, Postgres"####,
        file: r####"docker-compose.yml"####,
        contents: r####"services:
  app:
    build: .
    ports:
      - "3000:3000"
    environment:
      DATABASE_URL: postgres://${DB_USER:-app}:${DB_PASSWORD:?set DB_PASSWORD in .env}@db:5432/${DB_NAME:-app}
    depends_on:
      db:
        condition: service_healthy

  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: ${DB_USER:-app}
      POSTGRES_PASSWORD: ${DB_PASSWORD:?set DB_PASSWORD in .env}
      POSTGRES_DB: ${DB_NAME:-app}
    ports:
      - "127.0.0.1:5432:5432"
    volumes:
      - db:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U $${POSTGRES_USER}"]
      interval: 5s
      retries: 10

volumes:
  db:
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"docker-compose: Postgres, Redis"####,
        file: r####"docker-compose.yml"####,
        contents: r####"services:
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: ${DB_USER:-app}
      POSTGRES_PASSWORD: ${DB_PASSWORD:?set DB_PASSWORD in .env}
      POSTGRES_DB: ${DB_NAME:-app}
    ports:
      - "127.0.0.1:5432:5432"
    volumes:
      - db:/var/lib/postgresql/data

  redis:
    image: redis:7-alpine
    ports:
      - "127.0.0.1:6379:6379"

volumes:
  db:
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"nginx.conf for PHP-FPM"####,
        file: r####"nginx.conf"####,
        contents: r####"server {
    listen 80;
    root /var/www/html/public;
    index index.php;

    location / {
        try_files $uri $uri/ /index.php?$query_string;
    }

    location ~ \.php$ {
        fastcgi_pass app:9000;
        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
        include fastcgi_params;
    }

    location ~ /\. {
        deny all;
    }
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".editorconfig"####,
        file: r####".editorconfig"####,
        contents: r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
        executable: false,
    },
    FileTemplate {
        label: r####".env.example"####,
        file: r####".env.example"####,
        contents: r####"APP_ENV=local
DB_DATABASE=app
DB_USERNAME=app
DB_PASSWORD=
DB_ROOT_PASSWORD=
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"README.md"####,
        file: r####"README.md"####,
        contents: r####"# {{name}}

## Getting started

## Tests
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Makefile"####,
        file: r####"Makefile"####,
        contents: r####".PHONY: build test run clean

build:
	@echo build

test:
	@echo test

run:
	@echo run

clean:
	@echo clean
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"LICENSE: MIT"####,
        file: r####"LICENSE"####,
        contents: r####"MIT License

Copyright (c) [year] [name]

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"GitHub Actions: Rust"####,
        file: r####".github/workflows/ci.yml"####,
        contents: r####"name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"GitHub Actions: PHP"####,
        file: r####".github/workflows/ci.yml"####,
        contents: r####"name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: shivammathur/setup-php@v2
        with:
          php-version: "8.3"
      - run: composer install --prefer-dist --no-interaction
      - run: vendor/bin/phpunit
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"GitHub Actions: Node"####,
        file: r####".github/workflows/ci.yml"####,
        contents: r####"name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: npm ci
      - run: npm test
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"PHP class"####,
        file: r####"{{ask}}.php"####,
        contents: r####"<?php

declare(strict_types=1);

final class {{name}}
{
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"PHP interface"####,
        file: r####"{{ask}}.php"####,
        contents: r####"<?php

declare(strict_types=1);

interface {{name}}
{
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"PHPUnit test"####,
        file: r####"{{ask}}Test.php"####,
        contents: r####"<?php

declare(strict_types=1);

use PHPUnit\Framework\TestCase;

final class {{name}} extends TestCase
{
    public function testExample(): void
    {
        $this->assertTrue(true);
    }
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Python module"####,
        file: r####"{{ask}}.py"####,
        contents: r####""""{{name}}."""
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Python test"####,
        file: r####"test_{{ask}}.py"####,
        contents: r####"def test_example():
    assert True
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"JavaScript module"####,
        file: r####"{{ask}}.js"####,
        contents: r####"export function {{name}}() {
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"TypeScript module"####,
        file: r####"{{ask}}.ts"####,
        contents: r####"export function {{name}}(): void {
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"React component"####,
        file: r####"{{ask}}.tsx"####,
        contents: r####"type {{name}}Props = {};

export function {{name}}({}: {{name}}Props) {
  return <div />;
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Go file"####,
        file: r####"{{ask}}.go"####,
        contents: r####"package {{folder}}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Go test"####,
        file: r####"{{ask}}_test.go"####,
        contents: r####"package {{folder}}

import "testing"

func TestExample(t *testing.T) {
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Rust module"####,
        file: r####"{{ask}}.rs"####,
        contents: r####"//! {{name}}.
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Java class"####,
        file: r####"{{ask}}.java"####,
        contents: r####"public class {{name}} {
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Kotlin class"####,
        file: r####"{{ask}}.kt"####,
        contents: r####"class {{name}}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"Bash script"####,
        file: r####"{{ask}}.sh"####,
        contents: r####"#!/usr/bin/env bash
set -euo pipefail

"####,
        executable: true,
    },
    FileTemplate {
        label: r####"HTML page"####,
        file: r####"{{ask}}.html"####,
        contents: r####"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{name}}</title>
</head>
<body>
</body>
</html>
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"CSS file"####,
        file: r####"{{ask}}.css"####,
        contents: r####""####,
        executable: false,
    },
    FileTemplate {
        label: r####"JSON file"####,
        file: r####"{{ask}}.json"####,
        contents: r####"{
}
"####,
        executable: false,
    },
    FileTemplate {
        label: r####"SQL file"####,
        file: r####"{{ask}}.sql"####,
        contents: r####""####,
        executable: false,
    },
];

pub const PROJECT_TEMPLATES: &[ProjectTemplate] = &[
    ProjectTemplate {
        label: r####"Empty project"####,
        files: &[
            (
                r####"README.md"####,
                r####"# {{name}}
"####,
            ),
            (
                r####".gitignore"####,
                r####".DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Rust: application"####,
        files: &[
            (
                r####"Cargo.toml"####,
                r####"[package]
name = "{{name}}"
version = "0.1.0"
edition = "2024"

[dependencies]
"####,
            ),
            (
                r####"src/main.rs"####,
                r####"fn main() {
    println!("Hello, {{name}}!");
}

#[cfg(test)]
mod tests {
    #[test]
    fn works() {
        assert_eq!(2 + 2, 4);
    }
}
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
cargo run
cargo test
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"/target
**/*.rs.bk
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Rust: library"####,
        files: &[
            (
                r####"Cargo.toml"####,
                r####"[package]
name = "{{name}}"
version = "0.1.0"
edition = "2024"

[dependencies]
"####,
            ),
            (
                r####"src/lib.rs"####,
                r####"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 2), 4);
    }
}
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
cargo test
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"/target
**/*.rs.bk
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"PHP: Composer"####,
        files: &[
            (
                r####"composer.json"####,
                r####"{
    "name": "app/{{name}}",
    "description": "{{name}}",
    "type": "project",
    "license": "proprietary",
    "require": {
        "php": ">=8.3"
    },
    "require-dev": {
        "phpunit/phpunit": "^11.0"
    },
    "autoload": {
        "psr-4": {
            "App\\": "src/"
        }
    },
    "autoload-dev": {
        "psr-4": {
            "App\\Tests\\": "tests/"
        }
    }
}
"####,
            ),
            (
                r####"src/Greeter.php"####,
                r####"<?php

declare(strict_types=1);

namespace App;

final class Greeter
{
    public function greet(string $name): string
    {
        return "Hello, {$name}!";
    }
}
"####,
            ),
            (
                r####"tests/GreeterTest.php"####,
                r####"<?php

declare(strict_types=1);

namespace App\Tests;

use App\Greeter;
use PHPUnit\Framework\TestCase;

final class GreeterTest extends TestCase
{
    public function testGreets(): void
    {
        $this->assertSame('Hello, you!', (new Greeter())->greet('you'));
    }
}
"####,
            ),
            (
                r####"public/index.php"####,
                r####"<?php

declare(strict_types=1);

require __DIR__ . '/../vendor/autoload.php';

echo (new App\Greeter())->greet('{{name}}');
"####,
            ),
            (
                r####"phpunit.xml"####,
                r####"<?xml version="1.0" encoding="UTF-8"?>
<phpunit bootstrap="vendor/autoload.php" colors="true">
    <testsuites>
        <testsuite name="Tests">
            <directory>tests</directory>
        </testsuite>
    </testsuites>
</phpunit>
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
composer install
vendor/bin/phpunit
php -S localhost:8000 -t public
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"/vendor/
.phpunit.result.cache
.phpunit.cache/
.php-cs-fixer.cache
composer.phar
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Node: JavaScript"####,
        files: &[
            (
                r####"package.json"####,
                r####"{
  "name": "{{name}}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "start": "node src/index.js",
    "test": "node --test"
  }
}
"####,
            ),
            (
                r####"src/index.js"####,
                r####"export function greet(name) {
  return `Hello, ${name}!`;
}

console.log(greet('{{name}}'));
"####,
            ),
            (
                r####"test/index.test.js"####,
                r####"import assert from 'node:assert/strict';
import test from 'node:test';
import { greet } from '../src/index.js';

test('greets', () => {
  assert.equal(greet('you'), 'Hello, you!');
});
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
npm start
npm test
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"node_modules/
dist/
build/
coverage/
*.log
.npm/
.eslintcache
*.tsbuildinfo
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Node: TypeScript"####,
        files: &[
            (
                r####"package.json"####,
                r####"{
  "name": "{{name}}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "tsc",
    "start": "node dist/index.js"
  },
  "devDependencies": {
    "typescript": "^5.0.0"
  }
}
"####,
            ),
            (
                r####"tsconfig.json"####,
                r####"{
  "compilerOptions": {
    "target": "es2022",
    "module": "nodenext",
    "moduleResolution": "nodenext",
    "strict": true,
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
"####,
            ),
            (
                r####"src/index.ts"####,
                r####"export function greet(name: string): string {
  return `Hello, ${name}!`;
}

console.log(greet('{{name}}'));
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
npm install
npm run build
npm start
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"node_modules/
dist/
build/
coverage/
*.log
.npm/
.eslintcache
*.tsbuildinfo
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Python"####,
        files: &[
            (
                r####"pyproject.toml"####,
                r####"[project]
name = "{{name}}"
version = "0.1.0"
requires-python = ">=3.10"
dependencies = []

[project.optional-dependencies]
dev = ["pytest"]

[tool.pytest.ini_options]
pythonpath = ["src"]
"####,
            ),
            (
                r####"src/{{module}}/__init__.py"####,
                r####"def greet(name: str) -> str:
    return f"Hello, {name}!"
"####,
            ),
            (
                r####"src/{{module}}/__main__.py"####,
                r####"from {{module}} import greet

print(greet("{{name}}"))
"####,
            ),
            (
                r####"tests/test_greet.py"####,
                r####"from {{module}} import greet

def test_greets():
    assert greet("you") == "Hello, you!"
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
python3 -m venv .venv
.venv/bin/pip install -e '.[dev]'
.venv/bin/pytest
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"__pycache__/
*.py[cod]
.venv/
venv/
dist/
build/
*.egg-info/
.pytest_cache/
.mypy_cache/
.ruff_cache/
.coverage
htmlcov/
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Go"####,
        files: &[
            (
                r####"go.mod"####,
                r####"module {{name}}

go 1.22
"####,
            ),
            (
                r####"main.go"####,
                r####"package main

import "fmt"

func greet(name string) string {
	return "Hello, " + name + "!"
}

func main() {
	fmt.Println(greet("{{name}}"))
}
"####,
            ),
            (
                r####"main_test.go"####,
                r####"package main

import "testing"

func TestGreet(t *testing.T) {
	if got := greet("you"); got != "Hello, you!" {
		t.Fatal(got)
	}
}
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
go run .
go test ./...
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"/bin/
*.exe
*.test
*.out
coverage.txt
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Java: Maven"####,
        files: &[
            (
                r####"pom.xml"####,
                r####"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>app</groupId>
  <artifactId>{{name}}</artifactId>
  <version>0.1.0</version>
  <properties>
    <maven.compiler.release>21</maven.compiler.release>
    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
  </properties>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>5.11.0</version>
      <scope>test</scope>
    </dependency>
  </dependencies>
</project>
"####,
            ),
            (
                r####"src/main/java/app/Main.java"####,
                r####"package app;

public class Main {
    public static String greet(String name) {
        return "Hello, " + name + "!";
    }

    public static void main(String[] args) {
        System.out.println(greet("{{name}}"));
    }
}
"####,
            ),
            (
                r####"src/test/java/app/MainTest.java"####,
                r####"package app;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class MainTest {
    @Test
    void greets() {
        assertEquals("Hello, you!", Main.greet("you"));
    }
}
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}

```sh
mvn test
```
"####,
            ),
            (
                r####".gitignore"####,
                r####"target/
build/
.gradle/
*.class
*.jar
*.war
.DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
    ProjectTemplate {
        label: r####"Bash scripts"####,
        files: &[
            (
                r####"bin/{{name}}.sh"####,
                r####"#!/usr/bin/env bash
set -euo pipefail

echo "Hello, {{name}}!"
"####,
            ),
            (
                r####"README.md"####,
                r####"# {{name}}
"####,
            ),
            (
                r####".gitignore"####,
                r####".DS_Store
Thumbs.db
.idea/
.vscode/
*.swp
.env
.env.local
"####,
            ),
            (
                r####".editorconfig"####,
                r####"root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 4

[*.{js,jsx,ts,tsx,json,yml,yaml,css,html}]
indent_size = 2

[Makefile]
indent_style = tab

[*.md]
trim_trailing_whitespace = false
"####,
            ),
        ],
    },
];

pub fn initial_input(template: &FileTemplate) -> &'static str {
    if template.file.contains("{{ask}}") {
        ""
    } else {
        template.file
    }
}

pub fn file_name(template: &FileTemplate, typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("Type a name".to_owned());
    }
    if !template.file.contains("{{ask}}") {
        return Ok(typed.to_owned());
    }
    let (folder, name) = typed.rsplit_once('/').map_or(("", typed), |(f, n)| (f, n));
    if name.contains('.') {
        return Ok(typed.to_owned());
    }
    let file = template.file.replace("{{ask}}", name);
    Ok(if folder.is_empty() {
        file
    } else {
        format!("{folder}/{file}")
    })
}

fn stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.split('.').next().unwrap_or(name);
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

pub fn render_file(template: &FileTemplate, path: &str, folder: &str) -> String {
    let package = stem(folder);
    template.contents.replace("{{name}}", &stem(path)).replace(
        "{{folder}}",
        if package.is_empty() { "main" } else { &package },
    )
}

pub fn package_name(folder: &str) -> String {
    let name: String = folder
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("app-{name}").trim_end_matches('-').to_owned()
    } else {
        name.to_owned()
    }
}

pub fn render_project(template: &ProjectTemplate, folder: &str) -> Vec<(String, String)> {
    let name = package_name(folder);
    let module = name.replace('-', "_");
    let fill = |text: &str| {
        text.replace("{{name}}", &name)
            .replace("{{module}}", &module)
    };
    template
        .files
        .iter()
        .map(|(path, contents)| (fill(path), fill(contents)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_left_unfilled() {
        for template in FILE_TEMPLATES {
            let text = render_file(template, "src/Thing.php", "src");
            assert!(!text.contains("{{"), "{}", template.label);
        }
        for template in PROJECT_TEMPLATES {
            for (path, text) in render_project(template, "My Project") {
                assert!(
                    !path.contains("{{") && !text.contains("{{"),
                    "{}: {path}",
                    template.label
                );
            }
        }
    }

    #[test]
    fn every_project_has_what_any_project_needs() {
        for template in PROJECT_TEMPLATES {
            let files = render_project(template, "x");
            for needed in ["README.md", ".gitignore", ".editorconfig"] {
                assert!(
                    files.iter().any(|(p, _)| p == needed),
                    "{}: {needed}",
                    template.label
                );
            }
        }
    }

    #[test]
    fn structured_files_parse() {
        for template in PROJECT_TEMPLATES {
            for (path, text) in render_project(template, "My Project") {
                if path.ends_with(".json") {
                    let parsed = serde_json::from_str::<serde_json::Value>(&text);
                    assert!(parsed.is_ok(), "{}: {path}", template.label);
                }
                if path.ends_with(".toml") {
                    assert!(
                        text.parse::<toml::Table>().is_ok(),
                        "{}: {path}",
                        template.label
                    );
                }
            }
        }
    }

    #[test]
    fn names_are_fit_for_package_managers_and_classes() {
        assert_eq!(package_name("My Cool_App!"), "my-cool-app");
        assert_eq!(package_name("123"), "app-123");
        assert_eq!(package_name("***"), "app");
        let python = PROJECT_TEMPLATES
            .iter()
            .find(|t| t.label == "Python")
            .unwrap();
        let files = render_project(python, "My App");
        assert!(files.iter().any(|(p, _)| p == "src/my_app/__init__.py"));
        let class = FILE_TEMPLATES
            .iter()
            .find(|t| t.label == "PHP class")
            .unwrap();
        assert!(render_file(class, "src/UserRepo.php", "src").contains("final class UserRepo"));
        let go = FILE_TEMPLATES
            .iter()
            .find(|t| t.label == "Go file")
            .unwrap();
        assert_eq!(render_file(go, "x.go", "server"), "package server\n");
    }

    #[test]
    fn a_typed_name_gets_the_template_s_shape() {
        let find = |label: &str| FILE_TEMPLATES.iter().find(|t| t.label == label).unwrap();
        let class = find("PHP class");
        assert_eq!(file_name(class, "User").unwrap(), "User.php");
        assert_eq!(file_name(class, "src/User").unwrap(), "src/User.php");
        assert_eq!(file_name(class, "User.inc").unwrap(), "User.inc");
        assert!(file_name(class, "  ").is_err());
        assert_eq!(
            file_name(find("Python test"), "x/parse").unwrap(),
            "x/test_parse.py"
        );
        let docker = find("Dockerfile: Node");
        assert_eq!(initial_input(docker), "Dockerfile");
        assert_eq!(
            file_name(docker, "docker/Dockerfile").unwrap(),
            "docker/Dockerfile"
        );
        assert_eq!(initial_input(class), "");
    }
}
