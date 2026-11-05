//! Installers from game repackers play music; this one plays its own. A short original
//! loop, synthesised on the spot so it costs no space in the setup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MM_STREAM_DONE;

const RATE: u32 = 44_100;
const STEP: f32 = 60.0 / 112.0 / 4.0;

/// Semitones above A2, per bar: root, third, fifth, seventh of each chord.
const CHORDS: [[i32; 4]; 8] = [
    [0, 3, 7, 10],
    [-4, 0, 3, 7],
    [3, 7, 10, 14],
    [-2, 2, 5, 9],
    [5, 8, 12, 15],
    [0, 3, 7, 12],
    [-4, 0, 3, 8],
    [-5, -1, 2, 7],
];

/// Sixteenth steps of the melody over eight bars, as chord tones. -1 rests.
const MELODY: [i8; 32] = [
    3, -1, 2, 1, 2, -1, 3, -1,
    2, -1, 1, 0, 1, -1, -1, -1,
    3, -1, 3, 2, 1, -1, 2, -1,
    0, -1, 1, 2, 3, -1, -1, -1,
];

fn pitch(semitones: i32) -> f32 {
    110.0 * 2f32.powf(semitones as f32 / 12.0)
}

fn pulse(phase: f32, duty: f32) -> f32 {
    if phase.fract() < duty { 1.0 } else { -1.0 }
}

fn triangle(phase: f32) -> f32 {
    let p = phase.fract();
    4.0 * (p - 0.5).abs() - 1.0
}

/// One stereo sample at time `t` seconds.
fn sample(t: f32) -> (f32, f32) {
    let step = (t / STEP) as usize;
    let into = (t / STEP).fract() * STEP;
    let bar = (step / 16) % 8;
    let chord = CHORDS[bar];

    let arp_note = chord[[0, 1, 2, 3, 2, 1][step % 6]] + 12;
    let arp = pulse(t * pitch(arp_note), 0.25) * (-into * 9.0).exp() * 0.32;

    let bass_note = chord[if step % 8 == 6 { 2 } else { 0 }] - 12;
    let bass = triangle(t * pitch(bass_note)) * (-into * 2.5).exp() * 0.75;

    let tone = MELODY[(step / 4) % 32];
    let mut lead = 0.0;
    if tone >= 0 && bar >= 2 {
        let since = t % (STEP * 4.0);
        let vibrato = 1.0 + 0.004 * (t * 34.0).sin();
        lead = pulse(t * pitch(chord[tone as usize] + 24) * vibrato, 0.5) * (-since * 2.8).exp() * 0.26;
    }

    let hat = if step.is_multiple_of(2) {
        let noise = ((t * 12345.678).sin() * 43758.547).fract();
        (noise * 2.0 - 1.0) * (-into * 70.0).exp() * 0.1
    } else {
        0.0
    };

    let left = arp * 0.8 + bass + lead * 1.1 + hat;
    let right = arp * 1.1 + bass + lead * 0.8 + hat;
    (left * 0.2, right * 0.2)
}

pub struct Player {
    pub muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Player {
    /// Starts playing `music`, or the setup's own tune if there is none.
    pub fn start(music: Option<Vec<i16>>, muted: bool) -> Self {
        let muted = Arc::new(AtomicBool::new(muted));
        let stop = Arc::new(AtomicBool::new(false));
        let (m, s) = (muted.clone(), stop.clone());
        std::thread::spawn(move || {
            let _ = play(music, &m, &s);
        });
        Player { muted, stop }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn play(music: Option<Vec<i16>>, muted: &AtomicBool, stop: &AtomicBool) -> windows::core::Result<()> {
    const FRAMES: usize = 4096;
    const BUFFERS: usize = 4;
    let format = WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 2,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 4,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut device = HWAVEOUT::default();
    // SAFETY: the handle and format outlive the call; no callback is registered.
    let opened = unsafe { waveOutOpen(Some(&mut device), WAVE_MAPPER, &format, None, None, CALLBACK_NULL) };
    if opened != 0 {
        return Ok(());
    }
    let _ = MM_STREAM_DONE;
    let mut buffers = vec![vec![0i16; FRAMES * 2]; BUFFERS];
    let mut headers: Vec<WAVEHDR> = buffers
        .iter_mut()
        .map(|b| WAVEHDR {
            lpData: windows::core::PSTR(b.as_mut_ptr() as *mut u8),
            dwBufferLength: (FRAMES * 4) as u32,
            ..Default::default()
        })
        .collect();
    let header_size = std::mem::size_of::<WAVEHDR>() as u32;
    let mut position = 0usize;
    let mut fade = 0f32;
    let mut fill = |buffer: &mut [i16]| {
        for frame in buffer.as_chunks_mut::<2>().0 {
            let target = if muted.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
            fade += (target - fade) * 0.0006;
            let (l, r) = match &music {
                Some(data) if !data.is_empty() => {
                    let i = (position * 2) % data.len();
                    (data[i] as f32 / 32768.0, data[(i + 1) % data.len()] as f32 / 32768.0)
                }
                _ => sample(position as f32 / RATE as f32),
            };
            frame[0] = (l * fade * 32767.0).clamp(-32767.0, 32767.0) as i16;
            frame[1] = (r * fade * 32767.0).clamp(-32767.0, 32767.0) as i16;
            position += 1;
        }
    };
    for (buffer, header) in buffers.iter_mut().zip(headers.iter_mut()) {
        fill(buffer);
        // SAFETY: `header` points at `buffer`; both stay alive and unmoved until reset below.
        unsafe {
            waveOutPrepareHeader(device, header, header_size);
            waveOutWrite(device, header, header_size);
        }
    }
    while !stop.load(Ordering::SeqCst) {
        let mut idle = true;
        for (buffer, header) in buffers.iter_mut().zip(headers.iter_mut()) {
            if header.dwFlags & WHDR_DONE != 0 {
                fill(buffer);
                idle = false;
                // SAFETY: the device has finished with this header.
                unsafe {
                    waveOutWrite(device, header, header_size);
                }
            }
        }
        if idle {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    // SAFETY: reset returns every buffer before the headers are released.
    unsafe {
        waveOutReset(device);
        for header in headers.iter_mut() {
            waveOutUnprepareHeader(device, header, header_size);
        }
        waveOutClose(device);
    }
    Ok(())
}
