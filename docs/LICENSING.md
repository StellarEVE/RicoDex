# Licensing and binary distribution decision

RicoDex source is intended to be licensed under Apache-2.0; see the repository's LICENSE.

The supplied product breakdown records an unresolved distribution issue: the FFmpeg shared build previously staged with Windows executables may not match the intended permissive licence configuration. Dynamic linking alone does not settle every FFmpeg and external codec licence obligation.

Before distributing bundled Windows binaries, the owner must choose and document:
- the exact FFmpeg build configuration and its licence notices;
- optional libraries and encoders to exclude;
- the guaranteed software encoding floor on machines without a working hardware encoder;
- notices and source-offer obligations, if any, applicable to that exact build.

Until that decision is documented and reviewed, CI may compile and test the project, but RicoDex must not publish bundled binary releases. This keeps source-code licensing separate from obligations of third-party binaries.