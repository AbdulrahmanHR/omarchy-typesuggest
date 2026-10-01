use rustix::fs::{MemfdFlags, memfd_create};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use tiny_skia::Pixmap;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Dispatch, QueueHandle};

/// Create a WlBuffer from a tiny-skia Pixmap and attach it to a surface. With `buffer_height`
/// taller than the pixmap, the pixmap is placed at the bottom and the rest stays transparent.
pub fn draw_pixmap_to_surface<T>(
    surface: &wl_surface::WlSurface,
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<T>,
    pixmap: &Pixmap,
    buffer_height: u32,
) -> Result<(), Box<dyn std::error::Error>>
where
    T: 'static + Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()>,
{
    let width = pixmap.width() as i32;
    let bar_height = pixmap.height() as i32;
    let height = i32::try_from(buffer_height)?.max(bar_height);
    let stride = width.checked_mul(4).ok_or("Pixmap width overflow")?;
    let size = (stride.checked_mul(height).ok_or("Pixmap size overflow")?) as usize;
    let bar_top = height - bar_height;

    // Create an anonymous memfd; it starts zero-filled (fully transparent) without using memory
    // for the untouched rows
    let fd = memfd_create("typesuggest-buf", MemfdFlags::CLOEXEC)?;
    let mut file = File::from(fd);
    file.set_len(size as u64)?;

    // Convert Pixmap RGBA to Wayland ARGB8888 (Little-endian: B, G, R, A)
    let rgba_data = pixmap.data();
    let mut argb_data = Vec::with_capacity(rgba_data.len());

    for chunk in rgba_data.as_chunks::<4>().0 {
        let r = chunk[0];
        let g = chunk[1];
        let b = chunk[2];
        let a = chunk[3];
        // In little-endian ARGB8888 byte order: [B, G, R, A]
        argb_data.push(b);
        argb_data.push(g);
        argb_data.push(r);
        argb_data.push(a);
    }

    file.seek(SeekFrom::Start((bar_top * stride) as u64))?;
    file.write_all(&argb_data)?;
    file.flush()?;

    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());

    surface.set_buffer_scale(2);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, bar_top, width, bar_height);
    surface.commit();

    // Pool can be destroyed once buffer is created
    pool.destroy();

    Ok(())
}

/// Hide the surface by attaching None and committing
pub fn hide_surface(surface: &wl_surface::WlSurface) {
    surface.attach(None, 0, 0);
    surface.commit();
}
