//! PC Monitor 2.0 rendering: the widget and its cards drawn into a
//! premultiplied RGBA pixmap with tiny-skia, text with swash (Bahnschrift's
//! variable weights and widths from the Windows fonts folder). No windows
//! here: the same pixels go to the widget, the taskbar, the overlay, the
//! dashboard, and to PNG snapshot tests.

// drawing calls take coordinates, sizes and colours: many plain arguments
#![allow(clippy::too_many_arguments)]

pub mod canvas;
pub mod cards;
pub mod format;
pub mod overlay;
pub mod report;
pub mod text;
pub mod theme;
pub mod widget;
