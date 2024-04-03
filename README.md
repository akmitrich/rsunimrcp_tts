# rsunimrcp_tts
Rust port of demo UniMRCP synth plugin

## Build
Make sure to satisfy [all the pre-requisits](https://github.com/akmitrich/rsunimrcp-sys#build) for `rsunimrcp-sys` crate.

```bash
$ cargo build --release
```

## Install
Put the file `librsunimrcp_tts.so` into `plugin/` folder of the UniMRCP server installation.