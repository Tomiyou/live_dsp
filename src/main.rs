use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use std::io;

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

    // 6. Start the stream loopback
    run_loopback(input_device, output_device)?;

    Ok(())
}

fn run_loopback(input_device: &cpal::Device, output_device: &cpal::Device) -> Result<()> {
    const MAX_HW_BUFFER_SIZE: usize = 4096;

    let default_input_config = input_device.default_input_config()?;
    let default_output_config = output_device.default_output_config()?;

    /* Check that sample formats match */
    if default_input_config.sample_format() != default_output_config.sample_format() {
        panic!(
            "Input and output device sample format are different: {} vs {}",
            default_input_config.sample_format(),
            default_output_config.sample_format()
        );
    }

    /* Check that sample rates match */
    if default_input_config.sample_rate() != default_output_config.sample_rate() {
        panic!(
            "Input and output device sample rate are different: {} vs {}",
            default_input_config.sample_rate(),
            default_output_config.sample_rate()
        );
    }

    let input_config: cpal::StreamConfig = default_input_config.into();
    let output_config: cpal::StreamConfig = default_output_config.into();

    /* Check that buffer */

    println!("\nStream Config:");
    println!(
        "Input:  {} Hz, {} channels, buffer size {:?}",
        input_config.sample_rate, input_config.channels, input_config.buffer_size
    );
    println!(
        "Output: {} Hz, {} channels, buffer size {:?}",
        output_config.sample_rate, output_config.channels, output_config.buffer_size
    );

    // Create a Ring Buffer with a capacity of 2x the buffer size to prevent underruns/overruns
    // We transfer f32 samples.
    let ring_buffer = HeapRb::<f32>::new(MAX_HW_BUFFER_SIZE * 2);
    let (mut producer, mut consumer) = ring_buffer.split();

    // --- Build Input Stream ---
    // We assume the input might be Mono or Stereo, but we only want to extract 1 channel to send.
    let input_channels = input_config.channels as usize;
    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);

    let default_input_config = input_device.default_input_config()?;
    match default_input_config.sample_format() {
        cpal::SampleFormat::F32 => println!("Have F32"),
        cpal::SampleFormat::I16 => println!("Have I16"),
        other => println!("Have this {}", other),
    }
    let input_stream = match input_device.default_input_config()?.sample_format() {
        cpal::SampleFormat::F32 => input_device.build_input_stream(
            &input_config,
            move |data: &[f32], _: &_| {
                println!("Have input data");
                // If input is empty, nothing to do
                if data.is_empty() {
                    return;
                }

                // data is interleaved [L, R, L, R...]
                // We iterate by frames (chunks of channel count)
                for frame in data.chunks(input_channels) {
                    // Take the first channel (Mono) and push to ringbuffer
                    let sample = frame[0];
                    let _ = producer.try_push(sample);
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => input_device.build_input_stream(
            &input_config,
            move |data: &[i16], _: &_| {
                println!("Have input data");
                if data.is_empty() {
                    return;
                }
                for frame in data.chunks(input_channels) {
                    // Convert i16 to f32 range [-1.0, 1.0]
                    let sample = (frame[0] as f32) / i16::MAX as f32;
                    let _ = producer.try_push(sample);
                }
            },
            err_fn,
            None,
        )?,
        f => anyhow::bail!("Unsupported input format: {:?}", f),
    };

    // --- Build Output Stream ---
    let output_channels = output_config.channels as usize;
    let output_stream = match output_device.default_output_config()?.sample_format() {
        cpal::SampleFormat::F32 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [f32], _: &_| {
                println!("Have output data");
                for frame in data.chunks_mut(output_channels) {
                    // Try to get a sample from the ringbuffer, otherwise silence
                    let sample = consumer.try_pop().unwrap_or(0.0);
                    println!("Have f32 sample: {}", sample);

                    // Copy that single sample to ALL output channels (e.g. Left and Right)
                    for out_sample in frame.iter_mut() {
                        *out_sample = sample;
                    }
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [i16], _: &_| {
                println!("Have output data");
                for frame in data.chunks_mut(output_channels) {
                    let sample_f32 = consumer.try_pop().unwrap_or(0.0);
                    let sample_i16 = (sample_f32 * i16::MAX as f32) as i16;
                    println!("Have i16 sample: {}", sample_i16);

                    for out_sample in frame.iter_mut() {
                        *out_sample = sample_i16;
                    }
                }
            },
            err_fn,
            None,
        )?,
        f => anyhow::bail!("Unsupported output format: {:?}", f),
    };

    println!("\nStreaming started... Press Enter to exit.");
    input_stream.play()?;
    output_stream.play()?;

    // Keep the main thread alive while streaming
    let mut _input = String::new();
    io::stdin().read_line(&mut _input)?;

    Ok(())
}
