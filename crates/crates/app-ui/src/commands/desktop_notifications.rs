use iced::Task;

use crate::model::Message;

pub(crate) fn publish_desktop_notification_command(summary: String, body: String) -> Task<Message> {
    Task::perform(
        async move {
            desktop_linux::publish_desktop_notification(&summary, &body)
                .await
                .map_err(|error| error.to_string())
        },
        Message::DesktopNotificationPublished,
    )
}
