use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

fn main() -> Result<()> {
    // 1. Setup Host
    let host = cpal::default_host();
    println!("Default Host: {}\n", host.id().name());

    // 2. Query and Collect Input Devices
    println!("--- Input Devices ---");
    let input_devices: Vec<_> = host.input_devices()?.collect();

    if input_devices.is_empty() {
        println!("No input devices found.");
        return Ok(());
    }

    for (index, device) in input_devices.iter().enumerate() {
        let description = device.description().unwrap();
        let config = device.default_input_config();
        match config {
            Ok(c) => println!(
                "[{}] {} (Default Rate: {} Hz)",
                index,
                description,
                c.sample_rate()
            ),
            Err(_) => println!("[{}] {} (Config unavailable)", index, description),
        }
    }

    // 3. User Input Selection
    println!("\nEnter the ID of the input device to use:");
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let selection: usize = selection
        .trim()
        .parse()
        .context("Please enter a valid number")?;

    if selection >= input_devices.len() {
        anyhow::bail!("Invalid device index.");
    }
    let input_device = &input_devices[selection];
    println!(
        "Selected input device: (id {:?}) {}",
        input_device.id(),
        input_device.description()?
    );

    // 4. Query and Collect Output Devices
    println!("--- Output Devices ---");
    let output_devices: Vec<_> = host.output_devices()?.collect();

    if output_devices.is_empty() {
        println!("No output devices found.");
        return Ok(());
    }

    for (index, device) in output_devices.iter().enumerate() {
        let description = device.description().unwrap();
        let config = device.default_output_config();
        match config {
            Ok(c) => println!(
                "[{}] {} (Default Rate: {} Hz)",
                index,
                description,
                c.sample_rate()
            ),
            Err(_) => println!("[{}] {} (Config unavailable)", index, description),
        }
    }

    // 5. User Output Selection
    println!("\nEnter the ID of the output device to use:");
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let selection: usize = selection
        .trim()
        .parse()
        .context("Please enter a valid number")?;

    if selection >= output_devices.len() {
        anyhow::bail!("Invalid device index.");
    }
    let output_device = &output_devices[selection];
    println!(
        "Selected output device: (id {:?}) {}",
        output_device.id(),
        output_device.description()?
    );

    let file_path = "synth_44100.wav";
    play_audio_file(output_device, file_path)?;

    Ok(())
}

fn play_audio_file(device: &cpal::Device, file_path: &str) -> Result<()> {
    // Open the WAV file
    let path = Path::new(file_path);
    let reader = BufReader::new(File::open(path).context("Failed to open file")?);
    let mut wav_reader = hound::WavReader::new(reader).context("Failed to parse WAV file")?;

    let wav_spec = wav_reader.spec();
    println!(
        "Playing {} ({} Hz, {} channels, {:?} format)...",
        file_path, wav_spec.sample_rate, wav_spec.channels, wav_spec.sample_format
    );

    // Get the device config
    let config = device.default_output_config()?;
    println!(
        "Output Config: {} Hz, {} channels",
        config.sample_rate(),
        config.channels()
    );

    // NOTE: This example assumes the WAV sample rate matches the device sample rate
    // as requested. In a production app, you would need a resampler here.
    // NOTE: We assume input is mono.

    // Collect samples into a vector (Simple loading into RAM)
    // We convert everything to f32 for simplicity in the stream handling
    let source: Vec<f32> = match wav_spec.sample_format {
        hound::SampleFormat::Float => wav_reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => wav_reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect(),
    };

    // Shared pointer to the sample index so the audio thread can track position
    let mut sample_idx: usize = 0;

    // Create the error callback
    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let output_channels = config.channels() as usize;

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(output_channels) {
                    let input_sample = if sample_idx < source.len() {
                        source[sample_idx]
                    } else {
                        0.0
                    };
                    sample_idx += 1;

                    for output_sample in frame.iter_mut() {
                        *output_sample = input_sample;
                    }
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.into(),
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(output_channels) {
                    let input_sample = if sample_idx < source.len() {
                        (source[sample_idx] * i16::MAX as f32) as i16
                    } else {
                        0
                    };
                    sample_idx += 1;

                    for output_sample in frame.iter_mut() {
                        *output_sample = input_sample;
                    }
                }
            },
            err_fn,
            None,
        )?,
        f => anyhow::bail!("Unsupported sample format: {:?}", f),
    };

    println!("Playing stream");
    stream.play()?;

    // Wait for playback to finish
    // We define "finished" as when the index reaches the end of the samples
    std::thread::sleep(std::time::Duration::from_millis(20000));

    Ok(())
}
