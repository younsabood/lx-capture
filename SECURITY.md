# Security Policy

## Supported versions

Security fixes target the latest code on the `main` branch and the latest published release when practical.

## Report a vulnerability

Please use [GitHub private vulnerability reporting](https://github.com/younsabood/lx-capture/security/advisories/new) when it is enabled. Include the affected version, impact, and a minimal reproduction that contains no private screen data. Do not publish exploit details in a public issue before maintainers have had a chance to investigate.

If private reporting is unavailable, contact the repository owner through [GitHub](https://github.com/younsabood).

## Security boundaries

LX Capture reads pixels from a local Windows desktop output through DXGI. It does not open network listeners, transmit frames, create files, or record audio. Applications that receive its pixels are responsible for obtaining appropriate consent, protecting captured data, and choosing where and how to store or transmit it.
