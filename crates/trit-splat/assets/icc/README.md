# Compact ICC profiles

These are the small ICC v4 matrix/TRC display profiles from
[saucecontrol/Compact-ICC-Profiles](https://github.com/saucecontrol/Compact-ICC-Profiles),
released under CC0-1.0.

- `sRGB-v4.icc`: sRGB primaries + sRGB parametric TRC.
- `DisplayP3-v4.icc`: Display P3 primaries + sRGB parametric TRC.
- `Rec2020-g24-v4.icc`: Rec.2020 primaries + gamma 2.4 TRC (BT.1886 without black offset).

`trit-splat` uses these as-is for the matching output encoding, and patches the
shared `rTRC`/`gTRC`/`bTRC` tag to a 16-byte `curv` gamma when the CLI selects a
non-sRGB transfer (`bt1886` or `raw`). The profile header, colorants, `wtpt` and
`chad` stay unchanged.
