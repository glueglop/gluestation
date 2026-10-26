use std::ffi::{CStr, CString};
use std::ptr;
use x11_dl::glx;
use x11_dl::xlib;

const VERTEX_SHADER_SRC: &str = r#"
#version 330 core
layout (location = 0) in vec2 aPos;
layout (location = 1) in vec2 aTexCoord;

out vec2 TexCoord;
uniform vec4 uCrop;

void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    float u = uCrop.x + aTexCoord.x * uCrop.z;
    float v = uCrop.y + (1.0 - aTexCoord.y) * uCrop.w;
    TexCoord = vec2(u, v);
}
"#;

const FRAGMENT_SHADER_SRC: &str = r#"
#version 330 core
out vec4 FragColor;
in vec2 TexCoord;

uniform sampler2D vramTexture;

void main() {
    vec4 c = texture(vramTexture, TexCoord);
    FragColor = vec4(c.rgb, 1.0);
}
"#;

unsafe fn check_shader(shader: u32, what: &str) {
    let mut ok: i32 = 0;
    gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut len: i32 = 0;
        gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len);
        let mut buf = vec![0u8; len.max(1) as usize];
        gl::GetShaderInfoLog(shader, len, ptr::null_mut(), buf.as_mut_ptr() as *mut _);
        eprintln!("{} shader compile error: {}", what, String::from_utf8_lossy(&buf));
    }
}

pub struct Renderer {
    xlib: xlib::Xlib,
    glx: glx::Glx,
    display: *mut xlib::Display,
    window: xlib::Window,
    context: glx::GLXContext,
    wm_delete_window: xlib::Atom,
    texture: u32,
    vao: u32,
    u_crop_loc: i32,
    pub tv_mode: bool,
}

impl Renderer {
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let xlib = xlib::Xlib::open().map_err(|e| format!("Failed to open Xlib: {:?}", e))?;
            let glx = glx::Glx::open().map_err(|e| format!("Failed to open GLX: {:?}", e))?;

            let display = (xlib.XOpenDisplay)(ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay failed".into());
            }

            let screen = (xlib.XDefaultScreen)(display);

            #[rustfmt::skip]
            let visual_attribs = [
                glx::GLX_X_RENDERABLE, 1,
                glx::GLX_DRAWABLE_TYPE, glx::GLX_WINDOW_BIT,
                glx::GLX_RENDER_TYPE, glx::GLX_RGBA_BIT,
                glx::GLX_X_VISUAL_TYPE, glx::GLX_TRUE_COLOR,
                glx::GLX_RED_SIZE, 8,
                glx::GLX_GREEN_SIZE, 8,
                glx::GLX_BLUE_SIZE, 8,
                glx::GLX_ALPHA_SIZE, 8,
                glx::GLX_DEPTH_SIZE, 24,
                glx::GLX_DOUBLEBUFFER, 1,
                0,
            ];

            let mut fb_count = 0;
            let fb_configs = (glx.glXChooseFBConfig)(
                display,
                screen,
                visual_attribs.as_ptr(),
                &mut fb_count,
            );
            if fb_configs.is_null() || fb_count == 0 {
                return Err("Failed to find suitable GLX FBConfig".into());
            }

            let fb_config = *fb_configs;
            (xlib.XFree)(fb_configs as *mut _);

            let vi = (glx.glXGetVisualFromFBConfig)(display, fb_config);
            if vi.is_null() {
                return Err("glXGetVisualFromFBConfig failed".into());
            }

            let root = (xlib.XRootWindow)(display, (*vi).screen);
            let mut swa: xlib::XSetWindowAttributes = std::mem::zeroed();
            swa.colormap = (xlib.XCreateColormap)(display, root, (*vi).visual, xlib::AllocNone);
            swa.event_mask = xlib::StructureNotifyMask | xlib::KeyPressMask;

            let window = (xlib.XCreateWindow)(
                display,
                root,
                0,
                0,
                width,
                height,
                0,
                (*vi).depth,
                xlib::InputOutput as u32,
                (*vi).visual,
                xlib::CWColormap | xlib::CWEventMask,
                &mut swa,
            );

            (xlib.XFree)(vi as *mut _);

            let title_c = CString::new(title).unwrap();
            (xlib.XStoreName)(display, window, title_c.as_ptr());

            let wm_delete_c = CString::new("WM_DELETE_WINDOW").unwrap();
            let mut wm_delete_window = (xlib.XInternAtom)(display, wm_delete_c.as_ptr(), 0);
            (xlib.XSetWMProtocols)(display, window, &mut wm_delete_window, 1);

            (xlib.XMapWindow)(display, window);

            let glx_create_context_attribs_arb: Option<
                unsafe extern "C" fn(
                    *mut xlib::Display,
                    glx::GLXFBConfig,
                    glx::GLXContext,
                    xlib::Bool,
                    *const i32,
                ) -> glx::GLXContext,
            > = {
                let proc_name = CString::new("glXCreateContextAttribsARB").unwrap();
                let addr = (glx.glXGetProcAddressARB)(proc_name.as_ptr() as *const u8);
                if addr.is_none() {
                    return Err("glXCreateContextAttribsARB not available".into());
                }
                std::mem::transmute(addr)
            };

            #[rustfmt::skip]
            let context_attribs = [
                0x2091, 3,
                0x2092, 3,
                0x9126, 0x00000001,
                0,
            ];

            let context = (glx_create_context_attribs_arb.unwrap())(
                display,
                fb_config,
                ptr::null_mut(),
                1,
                context_attribs.as_ptr(),
            );

            if context.is_null() {
                return Err("Failed to create OpenGL 3.3 Core Context".into());
            }

            (glx.glXMakeCurrent)(display, window, context);

            gl::load_with(|symbol| {
                let c_str = CString::new(symbol).unwrap();
                (glx.glXGetProcAddressARB)(c_str.as_ptr() as *const u8)
                    .map_or(ptr::null(), |p| p as *const _)
            });

            let swap_interval_ext: Option<
                unsafe extern "C" fn(*mut xlib::Display, xlib::Drawable, i32),
            > = {
                let proc_name = CString::new("glXSwapIntervalEXT").unwrap();
                let addr = (glx.glXGetProcAddressARB)(proc_name.as_ptr() as *const u8);
                std::mem::transmute(addr)
            };
            if let Some(swap_interval) = swap_interval_ext {
                swap_interval(display, window, 0);
            }

            let gl_ver = CStr::from_ptr(gl::GetString(gl::VERSION) as *const i8).to_string_lossy();
            let gl_rend = CStr::from_ptr(gl::GetString(gl::RENDERER) as *const i8).to_string_lossy();
            println!("\n=======================================================");
            println!("  Native GL 3.3 Context Initialized!");
            println!("  Version:  {}", gl_ver);
            println!("  Renderer: {}", gl_rend);
            println!("=======================================================\n");

            let vs = gl::CreateShader(gl::VERTEX_SHADER);
            let vs_c = CString::new(VERTEX_SHADER_SRC).unwrap();
            gl::ShaderSource(vs, 1, &vs_c.as_ptr(), ptr::null());
            gl::CompileShader(vs);
            check_shader(vs, "Vertex");

            let fs = gl::CreateShader(gl::FRAGMENT_SHADER);
            let fs_c = CString::new(FRAGMENT_SHADER_SRC).unwrap();
            gl::ShaderSource(fs, 1, &fs_c.as_ptr(), ptr::null());
            gl::CompileShader(fs);
            check_shader(fs, "Fragment");

            let program = gl::CreateProgram();
            gl::AttachShader(program, vs);
            gl::AttachShader(program, fs);
            gl::LinkProgram(program);
            gl::UseProgram(program);

            let crop_name = CString::new("uCrop").unwrap();
            let u_crop_loc = gl::GetUniformLocation(program, crop_name.as_ptr());

            #[rustfmt::skip]
            let vertices: [f32; 24] = [
                -1.0,  1.0,  0.0, 1.0,
                -1.0, -1.0,  0.0, 0.0,
                 1.0, -1.0,  1.0, 0.0,

                -1.0,  1.0,  0.0, 1.0,
                 1.0, -1.0,  1.0, 0.0,
                 1.0,  1.0,  1.0, 1.0,
            ];

            let mut vao = 0;
            let mut vbo = 0;
            gl::GenVertexArrays(1, &mut vao);
            gl::GenBuffers(1, &mut vbo);

            gl::BindVertexArray(vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (vertices.len() * 4) as isize,
                vertices.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            gl::VertexAttribPointer(0, 2, gl::FLOAT, gl::FALSE, 4 * 4, ptr::null());
            gl::EnableVertexAttribArray(0);

            gl::VertexAttribPointer(1, 2, gl::FLOAT, gl::FALSE, 4 * 4, (2 * 4) as *const _);
            gl::EnableVertexAttribArray(1);

            let mut texture = 0;
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(gl::TEXTURE_2D, texture);

            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);

            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as i32,
                1024,
                512,
                0,
                gl::RGBA,
                gl::UNSIGNED_SHORT_1_5_5_5_REV,
                ptr::null(),
            );

            Ok(Self {
                xlib,
                glx,
                display,
                window,
                context,
                wm_delete_window,
                texture,
                vao,
                u_crop_loc,
                tv_mode: true,
            })
        }
    }

    pub fn poll_events(&mut self) -> bool {
        unsafe {
            while (self.xlib.XPending)(self.display) > 0 {
                let mut event: xlib::XEvent = std::mem::zeroed();
                (self.xlib.XNextEvent)(self.display, &mut event);

                match event.get_type() {
                    xlib::ClientMessage => {
                        if event.client_message.data.get_long(0) == self.wm_delete_window as i64 {
                            return false;
                        }
                    }
                    xlib::KeyPress => {
                        let keysym = (self.xlib.XLookupKeysym)(&mut event.key, 0);
                        if keysym == 0xFF1B {
                            return false;
                        }
                        if keysym == 0xFF09 {
                            self.tv_mode = !self.tv_mode;
                            println!("  [VIEW] Toggled mode: {}", if self.tv_mode { "TV 4:3 Mode" } else { "Full 1024x512 VRAM View" });
                        }
                    }
                    _ => {}
                }
            }
        }
        true
    }

    pub fn render_vram(&self, vram: &[u16], disp_x: usize, disp_y: usize, disp_w: usize, disp_h: usize) {
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, self.texture);
            gl::TexSubImage2D(
                gl::TEXTURE_2D,
                0,
                0,
                0,
                1024,
                512,
                gl::RGBA,
                gl::UNSIGNED_SHORT_1_5_5_5_REV,
                vram.as_ptr() as *const _,
            );

            if self.tv_mode {
                let u_x = (disp_x as f32) / 1024.0;
                let u_y = (disp_y as f32) / 512.0;
                let u_w = (disp_w as f32) / 1024.0;
                let u_h = (disp_h as f32) / 512.0;
                gl::Uniform4f(self.u_crop_loc, u_x, u_y, u_w, u_h);
            } else {
                gl::Uniform4f(self.u_crop_loc, 0.0, 0.0, 1.0, 1.0);
            }

            gl::BindVertexArray(self.vao);
            gl::DrawArrays(gl::TRIANGLES, 0, 6);

            (self.glx.glXSwapBuffers)(self.display, self.window);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            (self.glx.glXMakeCurrent)(self.display, 0, ptr::null_mut());
            (self.glx.glXDestroyContext)(self.display, self.context);
            (self.xlib.XDestroyWindow)(self.display, self.window);
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}
