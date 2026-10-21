//! Nocturne design tokens: the chrome's grounds, ramps and radii (dark schemes; light schemes
//! keep the colours `Look` derives from the terminal scheme).

use iced::Color;

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Cards, dialogs, inputs, the picker.
pub const SURFACE: Color = hex(0x23_2532);
/// Sidebar, SFTP panel, editor columns: mix(bg 60 %, surface 40 %).
pub const PANEL: Color = hex(0x1b_1d2a);
/// Tab / title bar: mix(bg 70 %, black 30 %).
pub const BAR: Color = hex(0x0f_111b);
pub const TEXT: Color = hex(0xe9_e9ed);
pub const DANGER: Color = hex(0xe5_484d);

/// Neutral ramp, 100 … 900.
pub const N100: Color = hex(0xf3_f5fe);
pub const N300: Color = hex(0xcf_d3e5);
pub const N400: Color = hex(0xb2_b6ca);
pub const N500: Color = hex(0x93_97ab);
pub const N600: Color = hex(0x75_798c);
pub const N700: Color = hex(0x59_5d6c);
pub const N800: Color = hex(0x3f_424d);
pub const N900: Color = hex(0x29_2b31);

/// The single accent, and its ramp 100 … 900.
pub const ACCENT: Color = hex(0x91_84d9);
pub const A100: Color = hex(0xf5_f4ff);
pub const A200: Color = hex(0xe7_e5fe);
pub const A300: Color = hex(0xd2_cefd);
pub const A400: Color = hex(0xb5_abfc);
pub const A500: Color = hex(0x96_8ae0);
pub const A600: Color = hex(0x79_6cbf);
pub const A700: Color = hex(0x5d_5294);
pub const A800: Color = hex(0x42_3a6a);
pub const A900: Color = hex(0x2b_2741);

/// Radii.
pub const R_BTN: f32 = 7.0;
pub const R_MD: f32 = 8.0;
pub const R_TAB: f32 = 9.0;
pub const R_PANE: f32 = 10.0;
pub const R_PICKER: f32 = 12.0;
pub const R_DIALOG: f32 = 14.0;

/// `c` at `a` alpha.
pub const fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// The large elevation (picker, dialogs): a light ring plus ambient darkness.
pub fn shadow_lg() -> iced::Shadow {
    iced::Shadow {
        color: alpha(Color::BLACK, 0.65),
        offset: iced::Vector::new(0.0, 16.0),
        blur_radius: 40.0,
    }
}

/// The ring that goes with [`shadow_lg`].
pub fn ring_lg(radius: f32) -> iced::Border {
    iced::Border::default().rounded(radius).width(1).color(N500)
}
