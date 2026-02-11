use anyhow::{Context, Result, anyhow};
use cpal::{Device, SupportedBufferSize};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use std::cmp::max;
use std::io;

fn select_io_devices() -> Result<(Device, Device)> {
    // 1. Setup Host
    let host = cpal::default_host();
    println!("Default Host: {}\n", host.id().name());

    // 2. Query and Collect Input Devices
    println!("--- Input Devices ---");
    let input_devices: Vec<_> = host.input_devices()?.collect();

    if input_devices.is_empty() {
        return Err(anyhow!("No input devices found."));
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
    let input_device = input_devices[selection].clone();
    println!(
        "Selected input device: (id {:?}) {}",
        input_device.id(),
        input_device.description()?
    );

    // 4. Query and Collect Output Devices
    println!("--- Output Devices ---");
    let output_devices: Vec<_> = host.output_devices()?.collect();

    if output_devices.is_empty() {
        return Err(anyhow!("No output devices found."));
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
    let output_device = output_devices[selection].clone();
    println!(
        "Selected output device: (id {:?}) {}",
        output_device.id(),
        output_device.description()?
    );

    Ok((input_device, output_device))
}

fn main() -> Result<()> {
    let (input_device, output_device) = select_io_devices()?;

    // Call this multiple times to have multiple vocals
    run_loopback(&input_device, &output_device)?;
    // jack_loopback(&input_device, &output_device)?;

    Ok(())
}

fn run_loopback(input_device: &cpal::Device, output_device: &cpal::Device) -> Result<()> {
    let default_input_config = input_device.default_input_config()?;
    let default_output_config = output_device.default_output_config()?;

    let (input_min_buf, input_max_buf) = match default_input_config.buffer_size() {
        SupportedBufferSize::Range { min, max } => (*min, *max),
        SupportedBufferSize::Unknown => (1024, 1024),
    };
    let (output_min_buf, output_max_buf) = match default_output_config.buffer_size() {
        SupportedBufferSize::Range { min, max } => (*min, *max),
        SupportedBufferSize::Unknown => (1024, 1024),
    };
    let min_buf = max(input_min_buf, output_min_buf);
    let max_buf = max(input_max_buf, output_max_buf);

    println!("\nEnter buffer size, min: {}, max: {}. Default is: 1024", min_buf, max_buf);
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let buffer_size: u32 = selection
        .trim()
        .parse()
        .unwrap_or(1024);

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

    let mut input_config: cpal::StreamConfig = default_input_config.into();
    let mut output_config: cpal::StreamConfig = default_output_config.into();
    // TODO: Tole ga zjebe wtf
    // input_config.buffer_size = cpal::BufferSize::Fixed(buffer_size);
    // output_config.buffer_size = cpal::BufferSize::Fixed(buffer_size);

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
    let L_ring_buffer = HeapRb::<f32>::new(buffer_size as usize * 2);
    let R_ring_buffer = HeapRb::<f32>::new(buffer_size as usize * 2);
    let (mut L_producer, mut L_consumer) = L_ring_buffer.split();
    let (mut R_producer, mut R_consumer) = R_ring_buffer.split();

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
                println!("Have f32 input data ({}), data len: {}", input_channels, data.len());
                // If input is empty, nothing to do
                if data.is_empty() {
                    return;
                }

                // data is interleaved [L, R, L, R...]
                // We iterate by frames (chunks of channel count)
                if input_channels == 2 {
                    for frame in data.chunks(2) {
                        if let Err(_) = L_producer.try_push(frame[0]) {
                            eprintln!("L producer full");
                        }
                        if let Err(_) = R_producer.try_push(frame[1]) {
                            eprintln!("R producer full");
                        }
                    }
                } else if input_channels == 1 {
                    for sample in data.iter() {
                        if let Err(_) = L_producer.try_push(*sample) {
                            eprintln!("L producer full");
                        }
                        if let Err(_) = R_producer.try_push(*sample) {
                            eprintln!("R producer full");
                        }
                    }
                } else {
                    panic!("What the fuck are these input channels: {}", input_channels);
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => input_device.build_input_stream(
            &input_config,
            move |data: &[i16], _: &_| {
                panic!("Have i16 input data");
                // if data.is_empty() {
                //     return;
                // }
                // for frame in data.chunks(input_channels) {
                //     // Convert i16 to f32 range [-1.0, 1.0]
                //     let sample = (frame[0] as f32) / i16::MAX as f32;
                //     let _ = L_producer.try_push(sample);
                // }
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
                println!("filling f32 output data ({}), data len: {}", output_channels, data.len());
                // for frame in data.chunks_mut(output_channels) {
                //     // Try to get a sample from the ringbuffer, otherwise silence
                //     let sample = left_consumer.try_pop().unwrap_or(0.0);
                //     println!("Have f32 sample: {}", sample);

                //     // Copy that single sample to ALL output channels (e.g. Left and Right)
                //     for out_sample in frame.iter_mut() {
                //         *out_sample = sample;
                //     }
                // }

                // data is interleaved [L, R, L, R...]
                // We iterate by frames (chunks of channel count)
                if output_channels == 2 {
                    for frame in data.chunks_mut(2) {
                        frame[0] = L_consumer.try_pop().unwrap_or_else(|| {
                            eprintln!("L consumer empty");
                            0.0
                        });
                        frame[1] = R_consumer.try_pop().unwrap_or_else(|| {
                            eprintln!("R consumer empty");
                            0.0
                        });
                    }
                } else if output_channels == 1 {
                    for sample in data.iter_mut() {
                        *sample = L_consumer.try_pop().unwrap_or_else(|| {
                            eprintln!("L consumer empty");
                            0.0
                        });
                        R_consumer.try_pop().unwrap_or_else(|| {
                            eprintln!("R consumer empty");
                            0.0
                        });
                    }
                } else {
                    panic!("What the fuck are these input channels: {}", input_channels);
                }
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => output_device.build_output_stream(
            &output_config,
            move |data: &mut [i16], _: &_| {
                panic!("filling i16 output data");
                // for frame in data.chunks_mut(output_channels) {
                //     let sample_f32 = left_consumer.try_pop().unwrap_or(0.0);
                //     let sample_i16 = (sample_f32 * i16::MAX as f32) as i16;
                //     println!("Have i16 sample: {}", sample_i16);

                //     for out_sample in frame.iter_mut() {
                //         *out_sample = sample_i16;
                //     }
                // }
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
