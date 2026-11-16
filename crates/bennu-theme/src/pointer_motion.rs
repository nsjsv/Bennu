//! 指针最后一次移动的时刻。
//!
//! 窗口根部的指针记录器静默写入，等着「停多久才藏」的地方到点时自己读。移动本身不发消息：iced 每条
//! 消息都要重建、重画所有窗口，高回报率鼠标每秒三百多次。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct PointerMotion(Rc<Cell<Option<Instant>>>);

impl PointerMotion {
    pub fn record(&self, at: Instant) {
        self.0.set(Some(at));
    }

    pub fn last(&self) -> Option<Instant> {
        self.0.get()
    }
}
