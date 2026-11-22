# Release signatures

`<tag>.minisig` is the minisign signature of that release's `PC-Monitor.zip`,
made on Vlad's PC with the release key (`sign-release.bat`). Pushing one here
runs `.github/workflows/release-signature.yml`: it checks the signature against
the zip in the release and the public key in `crates/pcm-core/src/update.rs`,
then attaches it as `PC-Monitor.zip.minisig`. PC Monitor 2.0 installs an update
only with it.
