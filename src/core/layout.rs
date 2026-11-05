use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Columns,
    Rows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Pane(PaneId),
    Split { axis: Axis, children: Vec<Layout> },
}

impl Layout {
    pub fn split(&mut self, target: PaneId, new: PaneId, axis: Axis) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    children: vec![Self::Pane(target), Self::Pane(new)],
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split {
                axis: own,
                children,
            } => {
                if *own == axis
                    && let Some(i) = children.iter().position(|c| *c == Self::Pane(target))
                {
                    children.insert(i + 1, Self::Pane(new));
                    return true;
                }
                children.iter_mut().any(|c| c.split(target, new, axis))
            }
        }
    }

    pub fn remove(&mut self, target: PaneId) -> bool {
        let Self::Split { children, .. } = self else {
            return false;
        };
        let removed = if let Some(i) = children.iter().position(|c| *c == Self::Pane(target)) {
            children.remove(i);
            true
        } else {
            children.iter_mut().any(|c| c.remove(target))
        };
        if children.len() == 1 {
            *self = children.remove(0);
        }
        removed
    }

    pub fn panes(&self) -> Vec<PaneId> {
        match self {
            Self::Pane(id) => vec![*id],
            Self::Split { children, .. } => children.iter().flat_map(Self::panes).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PaneId = PaneId(1);
    const B: PaneId = PaneId(2);
    const C: PaneId = PaneId(3);

    #[test]
    fn same_axis_splits_stay_flat() {
        let mut l = Layout::Pane(A);
        l.split(A, B, Axis::Columns);
        l.split(A, C, Axis::Columns);
        assert_eq!(
            l,
            Layout::Split {
                axis: Axis::Columns,
                children: vec![Layout::Pane(A), Layout::Pane(C), Layout::Pane(B)]
            }
        );
    }

    #[test]
    fn cross_axis_split_nests() {
        let mut l = Layout::Pane(A);
        l.split(A, B, Axis::Columns);
        l.split(B, C, Axis::Rows);
        assert_eq!(l.panes(), [A, B, C]);
        assert!(l.remove(B));
        assert_eq!(
            l,
            Layout::Split {
                axis: Axis::Columns,
                children: vec![Layout::Pane(A), Layout::Pane(C)]
            }
        );
    }

    #[test]
    fn removing_down_to_one_pane_collapses_and_keeps_the_last() {
        let mut l = Layout::Pane(A);
        l.split(A, B, Axis::Rows);
        l.remove(A);
        assert_eq!(l, Layout::Pane(B));
        assert!(!l.remove(B));
    }
}
