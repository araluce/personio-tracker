//! Thin browser-automation layer over CDP.

pub mod launch;
pub mod locator;
pub mod session;

pub use launch::launch;
pub use locator::nth_by_xpath;
