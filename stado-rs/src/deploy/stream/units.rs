//! The two systemd units the session runs on: their exact bodies, and the
//! managed-service declarations that describe them to the registry.

use super::{SUNSHINE_CONFIG, SUNSHINE_PROGRAM, SUNSHINE_UNIT};
use super::{XORG_CONFIG, XORG_PROGRAM, XORG_UNIT};
use crate::deploy::service;
use crate::stream::schema::{DisplayStream, DISPLAY};
use crate::targets::ComputeTarget;

pub(super) fn xorg_systemd_unit() -> String {
    format!(
        "[Unit]\n\
         Description=Stado stream: X server on the declared board\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={XORG_PROGRAM} {DISPLAY} -config {XORG_CONFIG} -noreset -novtswitch -sharevts\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

pub(super) fn sunshine_systemd_unit(declaration: &DisplayStream) -> String {
    let cuda_device = declaration.gpu_uuid.as_deref().unwrap_or("0");
    format!(
        "[Unit]\n\
         Description=Stado stream: Sunshine encoding the session\n\
         After={XORG_UNIT}\n\
         Requires={XORG_UNIT}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         Environment=DISPLAY={DISPLAY}\n\
         Environment=HOME=/root\n\
         # The screen lives on the declared board, but Sunshine's NVENC path opens the\n\
         # driver's default device, so the first live session rendered on card 1 and\n\
         # encoded on card 0. Binding the encoder keeps the whole session on one board,\n\
         # which is the point of declaring one on a two-card host.\n\
         Environment=CUDA_VISIBLE_DEVICES={cuda_device}\n\
         ExecStartPre=/bin/sh -c 'for _ in $(seq 30); do /usr/bin/xdpyinfo -display {DISPLAY} >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1'\n\
         # openbox does not daemonise, so it belongs in the background: as an\n\
         # ExecStartPre it never returned and the unit sat in `activating` until the\n\
         # start timeout, which reads exactly like a crash that never happened.\n\
         ExecStartPre=/bin/sh -c 'setsid /usr/bin/openbox --replace --sm-disable &'\n\
         ExecStart={SUNSHINE_PROGRAM} {SUNSHINE_CONFIG}\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// Canonical managed-service declarations for the two native units installed
/// by [`install`](super::install()). The exact systemd bodies are retained
/// because the generic service renderer cannot express Sunshine's dependency
/// and session prelude.
pub fn managed_services(
    target: &ComputeTarget,
    declaration: &DisplayStream,
    managed_since: &str,
) -> [service::ManagedService; 2] {
    let mut xorg = service::systemd_service(
        &target.name,
        XORG_UNIT,
        &format!("/etc/systemd/system/{XORG_UNIT}"),
        service::SOURCE_REGISTRY,
        managed_since,
    );
    xorg.program = XORG_PROGRAM.to_string();
    xorg.args = vec![
        DISPLAY.to_string(),
        "-config".to_string(),
        XORG_CONFIG.to_string(),
        "-noreset".to_string(),
        "-novtswitch".to_string(),
        "-sharevts".to_string(),
    ];
    xorg.systemd_unit = xorg_systemd_unit();

    let mut sunshine = service::systemd_service(
        &target.name,
        SUNSHINE_UNIT,
        &format!("/etc/systemd/system/{SUNSHINE_UNIT}"),
        service::SOURCE_REGISTRY,
        managed_since,
    );
    sunshine.program = SUNSHINE_PROGRAM.to_string();
    sunshine.args = vec![SUNSHINE_CONFIG.to_string()];
    sunshine.env = [
        ("DISPLAY".to_string(), DISPLAY.to_string()),
        ("HOME".to_string(), "/root".to_string()),
        (
            "CUDA_VISIBLE_DEVICES".to_string(),
            declaration
                .gpu_uuid
                .clone()
                .unwrap_or_else(|| "0".to_string()),
        ),
    ]
    .into_iter()
    .collect();
    sunshine.systemd_unit = sunshine_systemd_unit(declaration);

    [xorg, sunshine]
}
