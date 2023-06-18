use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::{env, ffi, fs};
use std::ffi::{OsStr, OsString};
use std::fs::DirEntry;
use std::io;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;
use clap::Parser;

const DEFAULT_TEX_WIDTH: u32 = 1024;
const DEFAULT_TEX_HEIGHT: u32 = 512;
const DEFAULT_TEX_DEPTH: u32 = 1;
const DEFAULT_TEX_4D: u32 = 1;

const NSG_SPATIAL_DATA_HEADER: [u8; 16] = [
    0x4E, 0x53, 0x47, 0xFF, 0x53, 0x70, 0x61, 0x74, 0x69, 0x61, 0x6C, 0x00, 0x00, 0x00, 0x00, 0x00
];
const NSG_SPATIAL_DATA_DIM_HEADER: [u8; 4] = [
    0x44, 0x49, 0x4D, 0xFA
];
const NSG_SPATIAL_DATA_ATTR_HEADER: [u8; 4] = [
    0x41, 0x54, 0x52, 0xFA
];
const NSG_SPATIAL_DATA_DATA_HEADER: [u8; 4] = [
    0x44, 0x41, 0x54, 0xFA
];

struct Layer {
    name: String,
    filepath: PathBuf,
    image: DynamicImage,
}

fn read_layer_files(path: &PathBuf) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .expect("Invalid path")
        .map(|res| res.map(|dir| dir.path()))
        .filter_map(|path| path.ok())
        .filter(|path| path.extension().unwrap_or("".as_ref()).eq("png"))
        .collect()
}

fn init_layers(layer_files: Vec<PathBuf>) -> Vec<Layer> {
    let mut layers = Vec::new();
    layers.reserve(layer_files.len());

    for file in layer_files {
        let layer_name: String = file.file_stem().unwrap().to_string_lossy().as_ref().into();
        println!(
            "Opening layer {layer_name} from file {}...",
            file.to_str().unwrap()
        );

        let mut reader = image::io::Reader::open(&file).unwrap();
        let img = reader.with_guessed_format().unwrap().decode().unwrap();

        println!("Resizing layer {layer_name}...");
        let image = img.resize(DEFAULT_TEX_WIDTH, DEFAULT_TEX_HEIGHT, FilterType::Nearest);

        // let mut new_filepath = file.parent().unwrap().to_path_buf();
        // let mut filename = file.file_stem().unwrap().to_os_string();
        // filename.push("_resized.");
        // filename.push(file.extension().unwrap());
        // new_filepath.push(filename);

        // image
        //     .save(new_filepath)
        //     .expect("Could not save the resized image");

        println!("Layer {layer_name} has been created.");

        let layer = Layer {
            name: layer_name,
            filepath: file,
            image,
        };

        layers.push(layer);
    }

    layers
}

fn make_attribute_bytes(layers: &[Layer]) -> Box<[u8]> {
    let mut attribute_bytes: Vec<u8> = vec![];
    for layer in layers {
        attribute_bytes.extend_from_slice(NSG_SPATIAL_DATA_ATTR_HEADER.as_slice());
        attribute_bytes.extend_from_slice(layer.name.as_ref());
        // string termination
        attribute_bytes.push(0);
        // attribute size
        attribute_bytes.push(1);
        // attribute type (ESpatialDataTexelAttributeType::Byte)
        attribute_bytes.push(3);
    }
    attribute_bytes.into_boxed_slice()
}

fn get_texel_count() -> u32 {
    return DEFAULT_TEX_WIDTH * DEFAULT_TEX_HEIGHT * DEFAULT_TEX_DEPTH * DEFAULT_TEX_4D;
}

fn make_dimensions_bytes() -> Box<[u8]> {
    let mut bytes: Vec<u8> = vec![];
    bytes.extend_from_slice(NSG_SPATIAL_DATA_DIM_HEADER.as_slice());
    bytes.extend_from_slice(DEFAULT_TEX_WIDTH.to_le_bytes().as_slice());
    bytes.extend_from_slice(DEFAULT_TEX_HEIGHT.to_le_bytes().as_slice());
    bytes.extend_from_slice(DEFAULT_TEX_DEPTH.to_le_bytes().as_slice());
    bytes.extend_from_slice(DEFAULT_TEX_4D.to_le_bytes().as_slice());
    bytes.into_boxed_slice()
}

fn make_data_bytes(layers: &[Layer]) -> Box<[u8]> {
    let mut bytes: Vec<u8> = vec![];

    bytes.extend_from_slice(NSG_SPATIAL_DATA_DATA_HEADER.as_slice());
    let combined_size = layers.len() * DEFAULT_TEX_WIDTH as usize * DEFAULT_TEX_HEIGHT as usize;
    if combined_size > u32::MAX as usize {
        panic!("For now, data chunks larger than u32::MAX are unsupported");
    }
    bytes.extend_from_slice((combined_size as u32).to_le_bytes().as_slice());

    let texel_count = get_texel_count();
    for i in 0..texel_count {
        for layer in layers {
            let rgba = layer.image.get_pixel(i % DEFAULT_TEX_WIDTH, i / DEFAULT_TEX_WIDTH);
            bytes.push(rgba.0[0]);
        }
    }

    bytes.into_boxed_slice()
}

#[derive(Parser)]
struct CliArgs {
    /// Input directory which contains the layer files.
    #[arg()]
    directory: PathBuf,

    /// Output file name (placed inside the specified input directory)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() {
    let args = CliArgs::parse();

    println!("Trying to generate spatial data file using layers from directory {}...",
             args.directory.display());

    let layers = read_layer_files(&args.directory);
    if layers.is_empty() {
        eprintln!("Layers not found.");
        exit(1);
    }

    let mut layers = init_layers(layers);

    println!("Generating the spatial data file...");

    let mut spatial_data_bytes: Vec<u8> = vec![];
    spatial_data_bytes.extend_from_slice(NSG_SPATIAL_DATA_HEADER.as_slice());

    let dimensions_bytes = make_dimensions_bytes();
    spatial_data_bytes.extend_from_slice(&*dimensions_bytes);

    let attribute_bytes = make_attribute_bytes(&layers);
    spatial_data_bytes.extend_from_slice(&*attribute_bytes);

    let data_bytes = make_data_bytes(&layers);
    spatial_data_bytes.extend_from_slice(&*data_bytes);

    let mut spatial_data_path = args.directory.clone();
    spatial_data_path.push(args.output.unwrap_or(PathBuf::from("SpatialDataOutputFile.nsgsd")));
    if let Err(_) = fs::write(&spatial_data_path, spatial_data_bytes) {
        eprintln!("Could not save the spatial data file.");
        exit(1);
    }

    println!("File {} has been generated successfully!", spatial_data_path.display());
}
