All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- [#1] `Trace::stage` and `Trace::end` were running `Sanitizer::sanitize` inside the `Config::VALIDATE` branch, conflating two independent config flags. Now, these two flags are honored independently.

## [0.1.0] - 2026.05.23

Initial release.
