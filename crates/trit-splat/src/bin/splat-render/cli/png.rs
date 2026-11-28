//! RGB PNG 落盘：8-bit RGB、色域标记与 ICC。

#[path = "png/write.rs"]
mod write;

pub use write::write_png;

#[cfg(test)]
#[path = "png/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "png/icc_tests.rs"]
mod icc_tests;
