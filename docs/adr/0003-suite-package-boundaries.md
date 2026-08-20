# ADR 0003: Core Hypr-DE and optional suite applications

- Status: accepted
- Date: 2026-08-20

## Context

The Hypr-DE repository provides a coherent Hyprland session and is also the
most visible installation entry point for the wider service suite. Treating
every suite application as a Hypr-DE dependency makes the base session grow
with unrelated capabilities, starts services a user did not select, and
conflates repository publication with default installation.

Voice Dictation is useful within the suite and shares its contracts and UI
runtime, but speech recognition and focused-window text injection are not
required for a working desktop session.

## Decision

Hypr-DE package dependencies contain only components required to provide its
documented default session. Optional suite applications are independently
versioned, packaged, and published in the Mason Arch repository and their
project COPR repositories, but are not dependencies or weak dependencies of
`hypr-de`.

Hypr-DE must not preset, enable, or make setup success depend on a service
owned by an optional application. An optional application owns its package
lifecycle and service activation. Hypr-DE may retain inert compatibility
integration, such as a window rule or key binding, when that integration has
no startup cost and does not make the application present.

`hyprland-voice-dictation` is an optional suite application under this policy.
Installing or upgrading `hypr-de` does not install or activate it. Users may
install it explicitly from `[mason]` on Arch or
`solaris765/hyprland-voice-dictation` on Fedora.

## Consequences

- The base session has a smaller dependency and background-service footprint.
- Publishing a suite application does not silently broaden Hypr-DE.
- Optional applications must document their own explicit installation and
  activation path.
- Package manifests, installer repository enablement, user presets, and setup
  validation must agree on the boundary.
- Shared contracts and architecture participation do not imply default
  installation.

## Rejected alternatives

**Keep Voice Dictation as a weak dependency.** Fedora installs recommended
packages by default, so this would preserve the unwanted default installation
in normal use.

**Remove Voice Dictation from the suite repositories.** Distribution and
default installation are separate concerns. The application remains a
supported, released suite component.

**Create a monolithic suite-extras package.** Independent optional packages
allow users to select capabilities without pulling unrelated applications.
