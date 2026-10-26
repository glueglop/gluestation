mod audio;
mod bios;
mod bus;
mod cdrom;
mod cpu;
mod dma;
mod gte;
mod gpu;
mod ram;
mod renderer;
mod spu;
mod timers;

use audio::AudioDevice;
use bios::Bios;
use bus::Bus;
use cpu::Cpu;
use renderer::Renderer;
use std::env;
use std::fs::File;
use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

fn save_wav(filename: &str, samples: &[i16], sample_rate: u32) {
    let mut file = match File::create(filename) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to create {}: {}", filename, e);
            return;
        }
    };

    let data_len = (samples.len() * 2) as u32;
    let file_len = data_len + 36;
    let channels = 2u16;
    let bits_per_sample = 16u16;
    let byte_rate = sample_rate * (channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);

    let _ = file.write_all(b"RIFF");
    let _ = file.write_all(&file_len.to_le_bytes());
    let _ = file.write_all(b"WAVEfmt ");
    let _ = file.write_all(&16u32.to_le_bytes());
    let _ = file.write_all(&1u16.to_le_bytes());
    let _ = file.write_all(&channels.to_le_bytes());
    let _ = file.write_all(&sample_rate.to_le_bytes());
    let _ = file.write_all(&byte_rate.to_le_bytes());
    let _ = file.write_all(&block_align.to_le_bytes());
    let _ = file.write_all(&bits_per_sample.to_le_bytes());
    let _ = file.write_all(b"data");
    let _ = file.write_all(&data_len.to_le_bytes());

    for s in samples {
        let _ = file.write_all(&s.to_le_bytes());
    }

    println!("\n  [AUDIO DUMP] Saved {} seconds of audio to '{}'!", samples.len() / (sample_rate as usize * 2), filename);
}

fn main() {
    let bios_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "SCPH1001.BIN".to_string());

    println!("Loading BIOS: {}", bios_path);

    let bios = match Bios::new(&bios_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error loading BIOS '{}': {}", bios_path, e);
            return;
        }
    };

    let mut bus = Bus::new(bios);
    let mut cpu = Cpu::new();

    let mut renderer = match Renderer::new("PlayStation 1 Emulator", 1024, 768) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to create X11/GLX renderer: {}", e);
            return;
        }
    };

    let mut audio_device = match AudioDevice::new() {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("Audio disabled: {}", e);
            None
        }
    };

    println!("cool thing start now :)))\n");

    let frame_duration = Duration::from_micros(16_667);
    let cycles_per_frame = 564_480;
    let samples_per_frame = 735;

    let mut frame: u32 = 0;
    let mut recorded_samples: Vec<i16> = Vec::with_capacity(44100 * 2 * 6);
    let mut wav_saved = false;

    loop {
        let frame_start = Instant::now();

        for _ in 0..cycles_per_frame {
            cpu.step(&mut bus);
            bus.tick(1);
        }

        frame += 1;

        bus.spu.step_samples(samples_per_frame);
        let samples = bus.spu.take_samples();

        if recorded_samples.len() < 44100 * 2 * 5 {
            recorded_samples.extend_from_slice(&samples);
        } else if !wav_saved {
            wav_saved = true;
            save_wav("chime.wav", &recorded_samples, 44100);
        }

        if let Some(ref mut audio) = audio_device {
            audio.write_samples(&samples);
        }

        bus.vblank();

        renderer.render_vram(
            &bus.gpu.vram[..],
            bus.gpu.disp_x,
            bus.gpu.disp_y,
            bus.gpu.disp_w,
            bus.gpu.disp_h,
        );

        if !renderer.poll_events() {
            println!("Window closed by user.");
            break;
        }

        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            thread::sleep(frame_duration - elapsed);
        }
    }

    if !wav_saved && !recorded_samples.is_empty() {
        save_wav("chime.wav", &recorded_samples, 44100);
    }
}
