use std::sync::atomic::{AtomicU32, Ordering};

use iced::advanced::widget::tree;

use super::*;

/// 有状态的行：状态是创建时领到的编号，看得出状态是不是还是原来那份。
struct Stamp;

static NEXT_STAMP: AtomicU32 = AtomicU32::new(0);

impl Widget<(), iced::Theme, ()> for Stamp {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<u32>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(NEXT_STAMP.fetch_add(1, Ordering::Relaxed))
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(10.0))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &(),
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(10.0), Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut (),
        _theme: &iced::Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}

fn rows(keys: std::ops::Range<u64>) -> Element<'static, (), iced::Theme, ()> {
    keyed_rows(keys.map(|key| (key, Element::new(Stamp))))
}

fn stamps(tree: &Tree) -> Vec<u32> {
    tree.children[0]
        .children
        .iter()
        .map(|row| *row.state.downcast_ref::<u32>())
        .collect()
}

/// 窗口往下挪两行：留下的行状态还是原来那份（排版缓存不丢），新进来的两行是新状态。
#[test]
fn rows_keep_their_state_when_the_window_moves() {
    let mut tree = Tree::new(rows(10..15));
    let before = stamps(&tree);

    tree.diff(rows(12..17));
    let after = stamps(&tree);

    assert_eq!(after[..3], before[2..]);
    assert!(after[3..].iter().all(|stamp| !before.contains(stamp)));

    // 往回挪：回来的行是新状态，一直在的行还是原来的。
    tree.diff(rows(11..16));
    let back = stamps(&tree);
    assert_eq!(back[1..], after[..4]);
    assert!(!after.contains(&back[0]));
}
