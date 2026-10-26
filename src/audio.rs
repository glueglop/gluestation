use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::ptr;

type SndPcmT = c_void;
type SndPcmHwParamsT = c_void;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const SND_PCM_FORMAT_S16_LE: c_int = 2;

unsafe extern "C" fn alsa_error_silencer(
    _file: *const c_char,
    _line: c_int,
    _function: *const c_char,
    _err: c_int,
    _fmt: *const c_char,
) {}

pub struct AudioDevice {
    handle: *mut SndPcmT,
    snd_pcm_writei: unsafe extern "C" fn(*mut SndPcmT, *const c_void, c_uint) -> isize,
    snd_pcm_recover: unsafe extern "C" fn(*mut SndPcmT, c_int, c_int) -> c_int,
    snd_pcm_close: unsafe extern "C" fn(*mut SndPcmT) -> c_int,
}

impl AudioDevice {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let lib = libc::dlopen(b"libasound.so.2\0".as_ptr() as *const c_char, libc::RTLD_LAZY);
            if lib.is_null() {
                return Err("Failed to dlopen libasound.so.2".into());
            }

            macro_rules! load_sym {
                ($name:ident, $ty:ty) => {{
                    let cname = CString::new(stringify!($name)).unwrap();
                    let sym = libc::dlsym(lib, cname.as_ptr());
                    if sym.is_null() {
                        return Err(format!("Missing ALSA symbol: {}", stringify!($name)));
                    }
                    std::mem::transmute::<*mut c_void, $ty>(sym)
                }};
            }

            let snd_lib_error_set_handler = load_sym!(
                snd_lib_error_set_handler,
                unsafe extern "C" fn(Option<unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char)>) -> c_int
            );

            snd_lib_error_set_handler(Some(alsa_error_silencer));

            let snd_pcm_open = load_sym!(
                snd_pcm_open,
                unsafe extern "C" fn(*mut *mut SndPcmT, *const c_char, c_int, c_int) -> c_int
            );
            let snd_pcm_hw_params_sizeof =
                load_sym!(snd_pcm_hw_params_sizeof, unsafe extern "C" fn() -> usize);
            let snd_pcm_hw_params_any = load_sym!(
                snd_pcm_hw_params_any,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT) -> c_int
            );
            let snd_pcm_hw_params_set_access = load_sym!(
                snd_pcm_hw_params_set_access,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_int) -> c_int
            );
            let snd_pcm_hw_params_set_format = load_sym!(
                snd_pcm_hw_params_set_format,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_int) -> c_int
            );
            let snd_pcm_hw_params_set_channels = load_sym!(
                snd_pcm_hw_params_set_channels,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_uint) -> c_int
            );
            let snd_pcm_hw_params_set_rate = load_sym!(
                snd_pcm_hw_params_set_rate,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_uint, c_int) -> c_int
            );
            let snd_pcm_hw_params_set_buffer_size = load_sym!(
                snd_pcm_hw_params_set_buffer_size,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_uint) -> c_int
            );
            let snd_pcm_hw_params_set_period_size = load_sym!(
                snd_pcm_hw_params_set_period_size,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT, c_uint, c_int) -> c_int
            );
            let snd_pcm_hw_params = load_sym!(
                snd_pcm_hw_params,
                unsafe extern "C" fn(*mut SndPcmT, *mut SndPcmHwParamsT) -> c_int
            );
            let snd_pcm_writei = load_sym!(
                snd_pcm_writei,
                unsafe extern "C" fn(*mut SndPcmT, *const c_void, c_uint) -> isize
            );
            let snd_pcm_recover = load_sym!(
                snd_pcm_recover,
                unsafe extern "C" fn(*mut SndPcmT, c_int, c_int) -> c_int
            );
            let snd_pcm_close = load_sym!(
                snd_pcm_close,
                unsafe extern "C" fn(*mut SndPcmT) -> c_int
            );

            let mut handle: *mut SndPcmT = ptr::null_mut();
            let targets: [&[u8]; 4] = [b"pipewire\0", b"pulse\0", b"default\0", b"sysdefault\0"];
            let mut opened = false;
            let mut last_err = 0;

            for target in targets {
                let ret = snd_pcm_open(&mut handle, target.as_ptr() as *const c_char, SND_PCM_STREAM_PLAYBACK, 0);
                if ret == 0 {
                    opened = true;
                    println!("  [AUDIO] Connected to ALSA device: {}", String::from_utf8_lossy(&target[..target.len()-1]));
                    break;
                }
                last_err = ret;
            }

            if !opened {
                return Err(format!("Could not open audio device (err {})", last_err));
            }

            let params_size = snd_pcm_hw_params_sizeof();
            let mut params_buf = vec![0u8; params_size];
            let params = params_buf.as_mut_ptr() as *mut SndPcmHwParamsT;

            snd_pcm_hw_params_any(handle, params);
            snd_pcm_hw_params_set_access(handle, params, SND_PCM_ACCESS_RW_INTERLEAVED);
            snd_pcm_hw_params_set_format(handle, params, SND_PCM_FORMAT_S16_LE);
            snd_pcm_hw_params_set_channels(handle, params, 2);
            snd_pcm_hw_params_set_rate(handle, params, 44100, 0);

            snd_pcm_hw_params_set_period_size(handle, params, 2048, 0);
            snd_pcm_hw_params_set_buffer_size(handle, params, 16384);

            let ret = snd_pcm_hw_params(handle, params);
            if ret < 0 {
                snd_pcm_close(handle);
                return Err(format!("snd_pcm_hw_params failed: err {}", ret));
            }

            let silence = vec![0i16; 4096];
            (snd_pcm_writei)(handle, silence.as_ptr() as *const c_void, 2048);

            println!("\n=======================================================");
            println!("  Native Audio Output Ready (44100 Hz Stereo)");
            println!("=======================================================\n");

            Ok(Self {
                handle,
                snd_pcm_writei,
                snd_pcm_recover,
                snd_pcm_close,
            })
        }
    }

    pub fn write_samples(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        let frames = (samples.len() / 2) as c_uint;
        unsafe {
            let written = (self.snd_pcm_writei)(self.handle, samples.as_ptr() as *const c_void, frames);
            if written < 0 {
                (self.snd_pcm_recover)(self.handle, written as c_int, 0);
            }
        }
    }
}

impl Drop for AudioDevice {
    fn drop(&mut self) {
        unsafe {
            (self.snd_pcm_close)(self.handle);
        }
    }
}
