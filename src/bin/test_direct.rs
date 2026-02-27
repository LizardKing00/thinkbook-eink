use image::{EncodableLayout, GrayImage};
use rust_it8951::{It8951, Mode};

fn main() -> anyhow::Result<()> {
    let mut it8951 = It8951::connect()?;
    let sys = it8951.get_system_info().unwrap();
    let w = sys.width;
    let h = sys.height;
    println!("{}x{}", w, h);

    let img_raw = image::open("/tmp/ready.png")?.to_luma8().as_bytes().to_vec();
    let img = image::DynamicImage::from(
        GrayImage::from_raw(w, h, img_raw).unwrap()
    );

    it8951.display_region(0, 0, w, h, Mode::INIT)?;
    it8951.load_region(&img, 0, 0)?;
    it8951.display_region(0, 0, w, h, Mode::GC16)?;
    Ok(())
}
