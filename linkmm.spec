%define _debugsource_template %{nil}
%define debug_package %{nil}

%bcond_without vendored

Name:           linkmm
Version:        0.1.0
Release:        2%{?dist}
Summary:        Link manager utility

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/linkmm
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz
%if %{with vendored}
Source1:        %{name}-%{version}-vendor.tar.zst
%endif

ExclusiveArch:  %{rust_arches}

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config

%description
linkmm is a Rust utility for managing symbolic links.

%prep
%autosetup -n %{name}-%{version}

%if %{with vendored}
# The vendor tarball (Source1) is generated via:
#   make copr
#
# It must contain:
#   vendor/
#   .cargo/config.toml
#
# The .cargo/config.toml file is produced by:
#   cargo vendor vendor > .cargo/config.toml
tar -xaf %{SOURCE1}
test -f .cargo/config.toml
test -d vendor
%endif

%build
export CARGO_HOME="$PWD/.cargo-home"
export RUSTFLAGS="%{?build_rustflags}"

# Optional local-only Cargo target cache.
# The Makefile passes this only for `make rpm` / `make ba` / `make rpm-local`
# / `make ba-local`. COPR and SRPM builds do not use it.
%if 0%{?_cargo_target_dir:1}
export CARGO_TARGET_DIR="%{_cargo_target_dir}"
%endif

%if %{with vendored}
export CARGO_NET_OFFLINE=true
cargo build --release --frozen --offline
%else
cargo build --release
%endif

%install
%if 0%{?_cargo_target_dir:1}
install -Dpm 0755 "%{_cargo_target_dir}/release/%{name}" "%{buildroot}%{_bindir}/%{name}"
%else
install -Dpm 0755 "target/release/%{name}" "%{buildroot}%{_bindir}/%{name}"
%endif

%check
test -x "%{buildroot}%{_bindir}/%{name}"

%files
%license LICENSE*
%doc README*
%{_bindir}/%{name}

%changelog
* Sun Apr 26 2026 sachesi <xsachesi@proton.me> - 0.1.0-2
- Add optional local-only Cargo target cache support for faster iterative RPM builds

* Sun Apr 26 2026 sachesi <xsachesi@proton.me> - 0.1.0-1
- Initial RPM package
