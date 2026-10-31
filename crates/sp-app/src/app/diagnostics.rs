//! Report actual adapter capabilities instead of implying hardware support.
use super::*;
impl ShopPhotoApp {
    pub(super) fn diagnostics_window(&mut self, ctx: &egui::Context) {
        if !self.state.diagnostics_open {
            return;
        }
        let mut open = true;
        egui::Window::new(crate::i18n::text("System & photo capabilities")).open(&mut open).default_width(570.0).show(ctx,|ui|{
            let mut report=format!("ShopPhoto {} · {} / {}\n",env!("CARGO_PKG_VERSION"),std::env::consts::OS,std::env::consts::ARCH);
            if let Some(gpu)=&self.gpu{
                let info=gpu.render_state.adapter.get_info();let limits=gpu.render_state.device.limits();
                report.push_str(&{ let __l10n_args = (&(info.name), &(info.backend), &(info.device_type), &(gpu.render_state.target_format), &(limits.max_texture_dimension_2d), &(limits.max_buffer_size/(1024*1024)),); crate::i18n::format("GPU: {0}\nBackend: {1} · {2}\nDisplay surface: {3}\nMaximum texture edge: {4} px\nMaximum buffer: {5} MiB\n", &[__l10n_args.0.to_string(), format!("{:?}", __l10n_args.1), format!("{:?}", __l10n_args.2), format!("{:?}", __l10n_args.3), __l10n_args.4.to_string(), __l10n_args.5.to_string()]) });
                if let Some((w,h))=gpu.renderer.proxy_size(){report.push_str(&{ let __l10n_args = (&(gpu.renderer.estimated_texture_bytes()as f64/(1024.0*1024.0)), &(sp_core::mask::alpha_storage_bytes(&self.state.edit)as f64/(1024.0*1024.0)),); crate::i18n::format("Develop proxy: {0} × {1}\nDevelop texture payload: {2} MiB\nCurrent edit alpha samples on CPU: {3} MiB\n", &[w.to_string(), h.to_string(), format!("{:.1}", __l10n_args.0), format!("{:.1}", __l10n_args.1)]) });}
                report.push_str(crate::i18n::text("Memory figures exclude driver padding, lookup tables, buffers, temporary patches, readback staging, history, other renderers, exports and AI allocations.\n"));
                report.push_str(crate::i18n::text(if gpu.hdr_surface{"Float linear scRGB surface active.\n"}else{"SDR surface active; HDR image exports remain available.\n"}));
                #[cfg(target_os="linux")]
                report.push_str(crate::i18n::text("Linux display peak luminance is not reported by this backend; verify compositor/display HDR before judging highlights.\n"));
            }else{report.push_str(crate::i18n::text("No compatible GPU is active.\n"));}
            report.push_str(&{ let __l10n_args = (&(crate::i18n::text(if cfg!(target_os="macos"){"macOS ImageIO (system codecs)"}else if cfg!(feature="native-codecs"){"compiled in (decoder plugin availability still applies)"}else{"not compiled in; use a native-codecs build"})), &(self.profiles.len()), &(self.lenses.len()),); crate::i18n::format("Native HEIF/AVIF input: {0}\nLoaded camera profiles: {1}\nLoaded lens profiles: {2}\n", &[__l10n_args.0.to_string(), __l10n_args.1.to_string(), __l10n_args.2.to_string()]) });
            report.push_str(crate::i18n::text("Raster input cap: 150 MP. Multi-photo inputs: 150 MP per frame / 320 MP total. Panorama canvas: 150 MP. Full-resolution exports use bounded tiles; source images, output buffers and retained masks still consume memory.\n"));
            ui.label(&report);if ui.button(crate::i18n::text("Copy capability report")).clicked(){ctx.copy_text(report);}
            ui.separator();ui.label(crate::i18n::text("AI runtime reports the actual interpreter, model cache and compute device. RAW denoise uses the installed ShopPhoto model."));
            #[cfg(target_os="linux")]
            ui.label(crate::i18n::text("Experimental Linux HDR: start under Wayland with SHOPPHOTO_LINUX_HDR=1. The Vulkan surface must advertise linear scRGB; otherwise the app retains SDR. SHOPPHOTO_SDR=1 forces SDR."));
        });
        self.state.diagnostics_open = open;
    }
}
