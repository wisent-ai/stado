//! Stage two, unit bodies: the exact systemd unit text for the remote queue
//! agent and for the remote diagnostics watchdog.

/// The remote agent systemd unit.
pub fn agent_unit_text(name: &str, stado_bin: &str, wc_python: &str, user: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Wisent Compute local GPU agent ({name})\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\nEnvironment=PYTHONUNBUFFERED=1\n\
         Environment=WC_PYTHON={wc_python}\n\
         ExecStart={stado_bin} agent --target {name}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         User={user}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// The remote diagnostics watchdog unit.
pub fn watchdog_unit_text(name: &str, watchdog_bin: &str, user: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Wisent Compute diagnostics watchdog ({name})\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\nEnvironment=PYTHONUNBUFFERED=1\n\
         ExecStart={watchdog_bin}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         User={user}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}
