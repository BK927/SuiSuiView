use super::candidates::{checked_pixel_count, expect_len, DecodedImage};
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use wuffs_sys::{
    sizeof__wuffs_gif__decoder, sizeof__wuffs_png__decoder, wuffs_base__decode_frame_options,
    wuffs_base__frame_config, wuffs_base__image_config, wuffs_base__io_buffer,
    wuffs_base__io_buffer_meta, wuffs_base__pixel_buffer, wuffs_base__pixel_config,
    wuffs_base__pixel_config__struct__bindgen_ty_1, wuffs_base__pixel_format,
    wuffs_base__pixel_subsampling, wuffs_base__range_ii_u64, wuffs_base__slice_u8,
    wuffs_base__status, wuffs_base__table_u8, wuffs_gif__decoder, wuffs_gif__decoder__decode_frame,
    wuffs_gif__decoder__decode_frame_config, wuffs_gif__decoder__decode_image_config,
    wuffs_gif__decoder__initialize, wuffs_gif__decoder__workbuf_len, wuffs_png__decoder,
    wuffs_png__decoder__decode_frame, wuffs_png__decoder__decode_frame_config,
    wuffs_png__decoder__decode_image_config, wuffs_png__decoder__initialize,
    wuffs_png__decoder__workbuf_len, WUFFS_BASE__PIXEL_FORMAT__RGBA_NONPREMUL,
    WUFFS_BASE__PIXEL_SUBSAMPLING__NONE, WUFFS_VERSION,
};

const WUFFS_INITIALIZE_ALREADY_ZEROED: u32 = 1;
const WUFFS_PIXEL_BLEND_SRC: u8 = 0;
const WUFFS_PIXEL_BLEND_SRC_OVER: u8 = 1;

pub fn decode_png(bytes: &[u8]) -> Result<DecodedImage, String> {
    unsafe {
        decode_with::<wuffs_png__decoder>(
            bytes,
            sizeof__wuffs_png__decoder,
            wuffs_png__decoder__initialize,
            wuffs_png__decoder__decode_image_config,
            wuffs_png__decoder__decode_frame_config,
            wuffs_png__decoder__workbuf_len,
            wuffs_png__decoder__decode_frame,
            WUFFS_PIXEL_BLEND_SRC,
        )
    }
}

pub fn decode_gif_first_frame(bytes: &[u8]) -> Result<DecodedImage, String> {
    unsafe {
        decode_with::<wuffs_gif__decoder>(
            bytes,
            sizeof__wuffs_gif__decoder,
            wuffs_gif__decoder__initialize,
            wuffs_gif__decoder__decode_image_config,
            wuffs_gif__decoder__decode_frame_config,
            wuffs_gif__decoder__workbuf_len,
            wuffs_gif__decoder__decode_frame,
            WUFFS_PIXEL_BLEND_SRC_OVER,
        )
    }
}

unsafe fn decode_with<T>(
    bytes: &[u8],
    sizeof_decoder: unsafe extern "C" fn() -> usize,
    initialize: unsafe extern "C" fn(*mut T, usize, u64, u32) -> wuffs_base__status,
    decode_image_config: unsafe extern "C" fn(
        *mut T,
        *mut wuffs_base__image_config,
        *mut wuffs_base__io_buffer,
    ) -> wuffs_base__status,
    decode_frame_config: unsafe extern "C" fn(
        *mut T,
        *mut wuffs_base__frame_config,
        *mut wuffs_base__io_buffer,
    ) -> wuffs_base__status,
    workbuf_len: unsafe extern "C" fn(*const T) -> wuffs_base__range_ii_u64,
    decode_frame: unsafe extern "C" fn(
        *mut T,
        *mut wuffs_base__pixel_buffer,
        *mut wuffs_base__io_buffer,
        u8,
        wuffs_base__slice_u8,
        *mut wuffs_base__decode_frame_options,
    ) -> wuffs_base__status,
    default_blend: u8,
) -> Result<DecodedImage, String> {
    let decoder_size = sizeof_decoder();
    let mut decoder = DecoderStorage::<T>::new(decoder_size)?;
    check_status(initialize(
        decoder.as_mut_ptr(),
        decoder_size,
        u64::from(WUFFS_VERSION),
        WUFFS_INITIALIZE_ALREADY_ZEROED,
    ))?;

    let mut src = io_buffer_from_bytes(bytes);
    let mut image_config = std::mem::zeroed::<wuffs_base__image_config>();
    check_status(decode_image_config(
        decoder.as_mut_ptr(),
        &mut image_config,
        &mut src,
    ))?;

    let width = image_config.pixcfg.private_impl.width;
    let height = image_config.pixcfg.private_impl.height;
    let pixel_count = checked_pixel_count(width, height)?;
    let mut pixels = vec![0u8; pixel_count * 4];
    let mut pixel_buffer = rgba_pixel_buffer(width, height, &mut pixels)?;

    let range = workbuf_len(decoder.as_mut_ptr());
    let workbuf_len = usize::try_from(range.max_incl)
        .map_err(|_| "Wuffs work buffer exceeds platform limits".to_owned())?;
    let mut workbuf = vec![0u8; workbuf_len];

    let mut frame_config = std::mem::zeroed::<wuffs_base__frame_config>();
    check_status(decode_frame_config(
        decoder.as_mut_ptr(),
        &mut frame_config,
        &mut src,
    ))?;

    let blend = if frame_config.private_impl.overwrite_instead_of_blend {
        WUFFS_PIXEL_BLEND_SRC
    } else {
        default_blend
    };
    check_status(decode_frame(
        decoder.as_mut_ptr(),
        &mut pixel_buffer,
        &mut src,
        blend,
        slice_from_mut(&mut workbuf),
        ptr::null_mut::<wuffs_base__decode_frame_options>(),
    ))?;

    expect_len(pixels.len(), pixel_count * 4, "Wuffs RGBA")?;
    Ok(DecodedImage::still(width, height, pixels))
}

fn io_buffer_from_bytes(bytes: &[u8]) -> wuffs_base__io_buffer {
    wuffs_base__io_buffer {
        data: wuffs_base__slice_u8 {
            // Wuffs treats the source buffer as read-only for decode paths even
            // though the C API type is mutable.
            ptr: bytes.as_ptr() as *mut u8,
            len: bytes.len(),
        },
        meta: wuffs_base__io_buffer_meta {
            wi: bytes.len(),
            ri: 0,
            pos: 0,
            closed: true,
        },
    }
}

fn rgba_pixel_buffer(
    width: u32,
    height: u32,
    pixels: &mut [u8],
) -> Result<wuffs_base__pixel_buffer, String> {
    let row_bytes = usize::try_from(width)
        .map_err(|_| "Wuffs width exceeds platform limits".to_owned())?
        .checked_mul(4)
        .ok_or_else(|| "Wuffs row byte count overflowed".to_owned())?;
    let height_usize =
        usize::try_from(height).map_err(|_| "Wuffs height exceeds platform limits".to_owned())?;
    expect_len(pixels.len(), row_bytes * height_usize, "Wuffs RGBA")?;

    let mut pixel_buffer = unsafe { std::mem::zeroed::<wuffs_base__pixel_buffer>() };
    pixel_buffer.pixcfg = wuffs_base__pixel_config {
        private_impl: wuffs_base__pixel_config__struct__bindgen_ty_1 {
            pixfmt: wuffs_base__pixel_format {
                repr: WUFFS_BASE__PIXEL_FORMAT__RGBA_NONPREMUL,
            },
            pixsub: wuffs_base__pixel_subsampling {
                repr: WUFFS_BASE__PIXEL_SUBSAMPLING__NONE,
            },
            width,
            height,
        },
    };
    pixel_buffer.private_impl.planes[0] = wuffs_base__table_u8 {
        ptr: pixels.as_mut_ptr(),
        width: row_bytes,
        height: height_usize,
        stride: row_bytes,
    };
    Ok(pixel_buffer)
}

fn slice_from_mut(bytes: &mut [u8]) -> wuffs_base__slice_u8 {
    wuffs_base__slice_u8 {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
    }
}

fn check_status(status: wuffs_base__status) -> Result<(), String> {
    if status.repr.is_null() {
        return Ok(());
    }
    let raw = unsafe { CStr::from_ptr(status.repr) }.to_bytes();
    if raw.first() == Some(&b'@') {
        return Ok(());
    }
    let message = unsafe { CStr::from_ptr(status.repr) }.to_string_lossy();
    let message = message
        .strip_prefix(['$', '#', '@'])
        .unwrap_or(message.as_ref());
    Err(message.to_owned())
}

struct DecoderStorage<T> {
    ptr: NonNull<T>,
    layout: Layout,
    _marker: PhantomData<T>,
}

impl<T> DecoderStorage<T> {
    fn new(size: usize) -> Result<Self, String> {
        if size == 0 {
            return Err("Wuffs decoder size was zero".to_owned());
        }
        // The generated Wuffs structs are opaque in bindgen, so allocate with
        // conservative alignment and pass the real C sizeof value to initialize.
        let layout = Layout::from_size_align(size, 64)
            .map_err(|_| "failed to create Wuffs decoder allocation layout".to_owned())?;
        let ptr = unsafe { alloc_zeroed(layout) as *mut T };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| "failed to allocate Wuffs decoder storage".to_owned())?;
        Ok(Self {
            ptr,
            layout,
            _marker: PhantomData,
        })
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }
}

impl<T> Drop for DecoderStorage<T> {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr.as_ptr().cast::<u8>(), self.layout);
        }
    }
}
