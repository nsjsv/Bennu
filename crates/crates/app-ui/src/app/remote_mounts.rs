use crate::app::FileBrowser;
use crate::network_connections::NetworkConnectionState;
use crate::sidebar_devices::SidebarDeviceState;

pub(crate) fn path_is_remote_mount(
    network_connections: &NetworkConnectionState,
    sidebar_devices: &SidebarDeviceState,
    path: &std::path::Path,
) -> bool {
    network_connections.path_is_mounted_network(path) || sidebar_devices.path_is_remote_mount(path)
}

impl FileBrowser {
    pub(crate) fn path_is_remote_mount(&self, path: &std::path::Path) -> bool {
        path_is_remote_mount(&self.network_connections, &self.sidebar_devices, path)
    }
}
