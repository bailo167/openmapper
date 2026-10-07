# Contributing to OpenMapper

All contributions are licensed under Apache-2.0.

Before submitting:
1. Read CLEANROOM.md.
2. Do not submit code or assets copied from proprietary applications.
3. Identify third-party material and its licence in THIRD_PARTY.yml.
4. Add tests for changed behaviour.
5. Run `cargo xtask ci`.
6. Update project-format migrations when persistent state changes.
7. Update documentation for externally visible behaviour.

Contributors must certify that they have the right to submit their work.

Pull requests fail automatically for:
- incompatible licences;
- architectural layer violations;
- missing project migrations;
- unauthorised golden changes;
- provenance-policy violations;
- failing three-platform builds.
