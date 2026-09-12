# Browser clipboard deployment

Deployed to https://dataclue.cn on 2026-09-12 UTC (2026-09-11 America/Los_Angeles).

Browser terminals use Cmd+V on macOS and Ctrl+V elsewhere for text and images. Images open a preview before upload; inserting the uploaded path into the current session is a separate action and does not send Enter. F5 refreshes the browser. Native terminals retain the F5 image shortcut.

## Release

- Image: `dc-workspace:0.2.14-browser-paste`
- Image ID: `sha256:1fefbc70e65e22d6bddf5c1ecc9b3aaa615c522c06f6e6369d387c1ad4a812a9`
- Manager image alias: `dc-workspace:0.2.14-publishing`
- Remote build directory: `/opt/dc-terminal/browser-paste`
- Backup: `/opt/dc-terminal/deployment/before-browser-paste-20260912-011038`
- Previous image alias: `dc-workspace:0.2.14-before-browser-paste`

The trusted classroom template at `/opt/dc-terminal/classroom/terminal.html` and the running workspace's HTML were replaced atomically. The existing workspace invokes the new binary through a launcher that sets `DCT_BROWSER_TERMINAL=1` for interactive launches. This avoided restarting ttyd, the daemon, or existing agents. New containers use the updated entrypoint and image. The separate project API image was unchanged.

Refresh existing browser tabs to load the update. The backup contains the previous template, workspace HTML, binary, compose configuration, image ID, and process snapshot. Restoring only an image tag does not undo the live workspace launcher or trusted template; restore those files as well if rolling back.

## Verification

- Rust UI tests: 507 passed.
- Browser clipboard tests: macOS, Windows, and Linux platform hints, preview, removal, upload errors/retry, explicit insertion, and no implicit Enter.
- Existing project panel, file manager, and publishing browser tests passed.
- Isolated release container: native macOS clipboard, real project upload/download, terminal insertion without Enter, and browser runtime hint verified.
- Production HTTPS: authenticated workspace, macOS native image paste/preview/upload/download, macOS and Windows footer hints, and student/admin role isolation passed.
- Existing production container start time and daemon/agent processes remained unchanged.
- Synthetic production upload and disposable validation container were removed. Production agents received no test input.
