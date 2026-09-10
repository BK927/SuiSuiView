//! SDR display transforms. The input is the already-composited sRGB scene,
//! never an image's embedded profile or an intermediate SR feature map.
use lcms2::{Intent, PixelFormat, Profile, Transform};

pub const LUT_EDGE: u32 = 65;

#[derive(Debug)]
pub struct DisplayLut {
    /// RGBA16Float, x = red, y = green, z = blue. Alpha is always one.
    pub rgba16: Vec<u16>,
}

pub fn build_display_lut(profile: &[u8]) -> Result<DisplayLut, String> {
    let destination = Profile::new_icc(profile).map_err(|error| error.to_string())?;
    let source = Profile::new_srgb();
    let transform = Transform::<[f32; 3], [f32; 3]>::new(
        &source,
        PixelFormat::RGB_FLT,
        &destination,
        PixelFormat::RGB_FLT,
        Intent::RelativeColorimetric,
    )
    .map_err(|error| error.to_string())?;
    let mut row = vec![[0.0; 3]; LUT_EDGE as usize];
    let mut rgba16 = Vec::with_capacity(LUT_EDGE.pow(3) as usize * 4);
    for blue in 0..LUT_EDGE {
        for green in 0..LUT_EDGE {
            for (red, pixel) in row.iter_mut().enumerate() {
                *pixel = [red as f32, green as f32, blue as f32]
                    .map(|value| value / (LUT_EDGE - 1) as f32);
            }
            transform.transform_in_place(&mut row);
            for pixel in &row {
                if pixel.iter().any(|value| !value.is_finite()) {
                    return Err("The display profile produced non-finite colors".to_owned());
                }
                rgba16.extend(pixel.map(unit_to_half));
                rgba16.push(0x3c00);
            }
        }
    }
    Ok(DisplayLut { rgba16 })
}

// This conversion only accepts finite unit-range values after clamping. Round
// to nearest/even in both normal and subnormal binary16 ranges.
fn unit_to_half(value: f32) -> u16 {
    let value = value.clamp(0.0, 1.0);
    if value < 2.0_f32.powi(-14) {
        return (value * 16_777_216.0).round_ties_even() as u16;
    }
    let bits = value.to_bits();
    let rounded = bits + 0xfff + ((bits >> 13) & 1);
    ((rounded >> 13) - 0x1c000) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn from_half(value: u16) -> f32 {
        let exponent = (value >> 10) & 31;
        let fraction = (value & 1023) as f32;
        if exponent == 0 {
            fraction * 2.0_f32.powi(-24)
        } else {
            (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent as i32 - 15)
        }
    }

    #[test]
    fn display_lut_srgb_identity_and_half_boundaries() {
        for value in [0.0, 2.0_f32.powi(-24), 2.0_f32.powi(-14), 0.5, 1.0] {
            assert_eq!(from_half(unit_to_half(value)), value);
        }
        let profile = Profile::new_srgb().icc().unwrap();
        let lut = build_display_lut(&profile).unwrap();
        for blue in 0..LUT_EDGE as usize {
            for green in 0..LUT_EDGE as usize {
                for red in 0..LUT_EDGE as usize {
                    let offset = ((blue * LUT_EDGE as usize + green) * LUT_EDGE as usize + red) * 4;
                    for (channel, expected) in [red, green, blue].into_iter().enumerate() {
                        let error = (from_half(lut.rgba16[offset + channel])
                            - expected as f32 / (LUT_EDGE - 1) as f32)
                            .abs();
                        assert!(error < 0.001, "channel error {error}");
                    }
                    assert_eq!(lut.rgba16[offset + 3], 0x3c00);
                }
            }
        }
    }

    #[test]
    fn display_lut_rejects_invalid_profile() {
        assert!(build_display_lut(b"invalid ICC").is_err());
    }
}
