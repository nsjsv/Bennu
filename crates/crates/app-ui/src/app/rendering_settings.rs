use iced::Task;

use super::FileBrowser;
use crate::config::RenderingGpuPreference;
use crate::model::Message;
use crate::startup_rendering::{self, StartupRenderingEnvironment};

impl FileBrowser {
    pub(super) fn select_rendering_gpu_preference(
        &mut self,
        preference: RenderingGpuPreference,
    ) -> Task<Message> {
        if self.rendering_gpu_preference == preference {
            return Task::none();
        }

        self.rendering_gpu_preference = preference;
        self.user_config.rendering_gpu_preference = preference;
        self.pending_renderer_restart_environment =
            Some(StartupRenderingEnvironment::without_display_probe(
                preference,
                self.startup_rendering_environment.backend(),
            ));
        self.renderer_restart_notice_visible = true;
        self.persist_app_config_command()
    }

    pub(super) fn restart_with_selected_renderer(&mut self) -> Task<Message> {
        let Some(environment) = self.pending_renderer_restart_environment.clone() else {
            self.renderer_restart_notice_visible = false;
            return Task::none();
        };

        match startup_rendering::restart_current_process(&environment) {
            Ok(()) => Task::none(),
            Err(error) => {
                self.show_global_error(error);
                Task::none()
            }
        }
    }

    pub(super) fn dismiss_renderer_restart_notice(&mut self) -> Task<Message> {
        self.renderer_restart_notice_visible = false;
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::FileBrowser;
    use crate::config::{self, RenderingGpuPreference};
    use crate::startup_rendering::StartupRenderingBackend;

    #[test]
    fn selecting_gpu_preference_records_restart_environment() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.select_rendering_gpu_preference(RenderingGpuPreference::HighPerformanceGpu));

        assert_eq!(
            browser.rendering_gpu_preference,
            RenderingGpuPreference::HighPerformanceGpu
        );
        assert!(browser.renderer_restart_notice_visible);
        assert_eq!(
            browser
                .pending_renderer_restart_environment
                .as_ref()
                .expect("pending renderer environment")
                .backend(),
            StartupRenderingBackend::Gl
        );
    }
}
