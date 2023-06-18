use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc;
use std::time::Instant;

use clap::{Parser, ArgAction};
use image::{DynamicImage, GenericImageView};
use image::imageops::FilterType;
use thousands::Separable;
use threadpool::ThreadPool;

const NSD_HEADER: [u8; 16] = [
    0x4E, 0x53, 0x47, 0xFF, 0x53, 0x70, 0x61, 0x74, 0x69, 0x61, 0x6C, 0x00, 0x00, 0x00, 0x00, 0x00
];
const NSD_DIM_HEADER: [u8; 4] = [
    0x44, 0x49, 0x4D, 0xFA
];
const NSD_ATTR_HEADER: [u8; 4] = [
    0x41, 0x54, 0x52, 0xFA
];
const NSD_DATA_HEADER: [u8; 4] = [
    0x44, 0x41, 0x54, 0xFA
];

#[derive(Clone)]
struct LayerDimensions {
    width: u32,
    height: u32,
}

impl LayerDimensions {
    pub fn from_power_of_two(width_power_of_two: u32, height_power_of_two: u32) -> LayerDimensions {
        LayerDimensions {
            width: 2u32.pow(width_power_of_two),
            height: 2u32.pow(height_power_of_two),
        }
    }

    pub fn get_texel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

impl Default for LayerDimensions {
    fn default() -> Self {
        LayerDimensions {
            width: 1024,
            height: 512,
        }
    }
}

struct Layer {
    name: String,
    image: DynamicImage,
}

impl Layer {
    pub fn from_file(file: &PathBuf, dimensions: &LayerDimensions, save_resized: bool) -> Layer {
        let layer_name: String = file.file_stem().unwrap().to_string_lossy().as_ref().into();
        println!(
            "Opening layer {layer_name} from file {}...",
            file.to_str().unwrap()
        );

        let reader = image::io::Reader::open(&file).unwrap();
        let img = reader.with_guessed_format().unwrap().decode().unwrap();

        println!("Resizing layer {layer_name}...");
        let image = img.resize(dimensions.width, dimensions.height, FilterType::Nearest);

        if save_resized {
            let mut new_filepath = file.parent().unwrap().to_path_buf();
            new_filepath.push("_resized");
            new_filepath.push(file.file_name().unwrap());

            if let Err(_) = image.save(&new_filepath) {
                eprintln!("Could not save the resized image {}", new_filepath.display());
            }
        }

        println!("Layer {layer_name} has been created.");

        Layer {
            name: layer_name,
            image,
        }
    }
}

fn read_layer_files(path: &PathBuf) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .expect("Invalid path")
        .map(|res| res.map(|dir| dir.path()))
        .filter_map(|path| path.ok())
        .filter(|path| path.extension().unwrap_or("".as_ref()).eq("png"))
        .collect()
}

fn init_layers_parallel(layer_files: Vec<PathBuf>, dimensions: &LayerDimensions, save_resized: bool) -> Vec<Layer> {
    let jobs = layer_files.len();
    let available_workers = std::thread::available_parallelism().map_or(4usize, |threads| threads.get());
    let workers = std::cmp::min(jobs, available_workers);
    let pool = ThreadPool::new(workers);

    let (sender, receiver) = mpsc::channel();
    for file in layer_files {
        let s = sender.clone();
        let dimensions_cloned = dimensions.clone();
        pool.execute(move|| {
            s.send(Layer::from_file(&file, &dimensions_cloned, save_resized))
                .expect("The layer will never be sent.");
        });
    }

    receiver.iter().take(jobs).collect()
}

fn init_layers(layer_files: Vec<PathBuf>, dimensions: &LayerDimensions, mut save_resized: bool) -> Vec<Layer> {
    assert!(!layer_files.is_empty());

    if save_resized {
        let mut path = layer_files[0].parent().unwrap().to_path_buf();
        path.push("_resized");
        if let Err(_) = fs::create_dir(&path) {
            eprintln!("Could not create directory {}", path.display());
            save_resized = false;
        }
    }

    init_layers_parallel(layer_files, &dimensions, save_resized)
}

fn make_attribute_bytes(layers: &[Layer]) -> Box<[u8]> {
    let mut attribute_bytes: Vec<u8> = vec![];
    for layer in layers {
        attribute_bytes.extend_from_slice(NSD_ATTR_HEADER.as_slice());
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

fn make_dimensions_bytes(dimensions: &LayerDimensions) -> Box<[u8]> {
    let mut bytes: Vec<u8> = vec![];
    bytes.extend_from_slice(NSD_DIM_HEADER.as_slice());
    bytes.extend_from_slice(dimensions.width.to_le_bytes().as_slice());
    bytes.extend_from_slice(dimensions.height.to_le_bytes().as_slice());
    bytes.extend_from_slice(1u32.to_le_bytes().as_slice());
    bytes.extend_from_slice(1u32.to_le_bytes().as_slice());
    bytes.into_boxed_slice()
}

fn make_data_bytes(layers: &[Layer], dimensions: &LayerDimensions) -> Box<[u8]> {
    let mut bytes: Vec<u8> = vec![];

    bytes.extend_from_slice(NSD_DATA_HEADER.as_slice());
    let combined_size = layers.len() * dimensions.width as usize * dimensions.height as usize;
    if combined_size > u32::MAX as usize {
        panic!("For now, data chunks larger than u32::MAX are unsupported");
    }
    bytes.extend_from_slice((combined_size as u32).to_le_bytes().as_slice());

    let texel_count = dimensions.get_texel_count();
    for i in 0..texel_count {
        for layer in layers {
            let rgba = layer.image.get_pixel(i as u32 % dimensions.width, i as u32 / dimensions.width);
            bytes.push(rgba.0[0]);
        }
    }

    bytes.into_boxed_slice()
}

fn make_binary(layers: &[Layer], dimensions: &LayerDimensions) -> Vec<u8> {
    let mut bytes: Vec<u8> = vec![];
    bytes.extend_from_slice(NSD_HEADER.as_slice());

    let dimensions_bytes = make_dimensions_bytes(dimensions);
    bytes.extend_from_slice(&*dimensions_bytes);

    let attribute_bytes = make_attribute_bytes(layers);
    bytes.extend_from_slice(&*attribute_bytes);

    let data_bytes = make_data_bytes(layers, dimensions);
    bytes.extend_from_slice(&*data_bytes);

    bytes
}

#[derive(Parser)]
#[clap(disable_help_flag = true)]
struct CliArgs {
    #[arg(long, action = ArgAction::Help, help = "Show help")]
    help: Option<bool>,

    /// Input directory which contains the layer files.
    #[arg()]
    directory: PathBuf,

    /// Output file name (placed inside the specified input directory)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Texture width will be set to 2^wpower (min=0, max=12)
    #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(0..=12), value_name = "WIDTH_POWER")]
    wpower: u8,

    /// Texture height will be set to 2^hpower (min=0, max=12)
    #[arg(short, long, default_value_t = 9, value_parser = clap::value_parser!(u8).range(0..=12), value_name = "HEIGHT_POWER")]
    hpower: u8,

    #[arg(long, default_value_t = false)]
    save_resized: bool,
}

fn main() {
    let args = CliArgs::parse();

    println!("Trying to generate spatial data file using layers from directory {}...",
             args.directory.display());

    let start = Instant::now();

    let layers = read_layer_files(&args.directory);
    if layers.is_empty() {
        eprintln!("Layers not found.");
        exit(1);
    }

    let dimensions = LayerDimensions::from_power_of_two(args.wpower as u32, args.hpower as u32);
    let layers = init_layers(layers, &dimensions, args.save_resized);

    println!("Generating the spatial data file...");

    let spatial_data_bytes = make_binary(layers.as_slice(), &dimensions);

    let mut spatial_data_path = args.directory.clone();
    spatial_data_path.push(args.output.unwrap_or(PathBuf::from("OutputFile.nsd")));
    if let Err(_) = fs::write(&spatial_data_path, spatial_data_bytes) {
        eprintln!("Could not save the spatial data file.");
        exit(1);
    }

    println!("File {} has been generated successfully!", spatial_data_path.display());

    let file_size = fs::metadata(&spatial_data_path)
        .map_or(0, |metadata| metadata.file_size())
        .separate_with_commas();
    let duration = (Instant::now() - start)
        .as_secs_f64();

    println!("Stats:");
    println!("    File size: {file_size} bytes");
    println!("    Time took: {duration:.5} seconds");
}
