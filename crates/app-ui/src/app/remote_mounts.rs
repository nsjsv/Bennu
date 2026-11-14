use crate::app::FileBrowser;
use crate::network_connections::NetworkConnectionState;
use crate::sidebar_devices::SidebarDeviceState;

pub(crate) fn path_is_remote_mount(
    network_connections: &NetworkConnectionState,
    sidebar_devices: &SidebarDeviceState,
    path: &std::path::Path,
) -> bool {
    bennu_sidebar::path_is_on_network(path, &network_connections.entries, &sidebar_devices.devices)
}

impl FileBrowser {
    pub(crate) fn path_is_remote_mount(&self, path: &std::path::Path) -> bool {
        path_is_remote_mount(&self.network_connections, &self.sidebar_devices, path)
    }
}
