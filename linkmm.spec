%define _debugsource_template %{nil}
%define debug_package %{nil}
Name:           linkmm
Version:        0.1.0
Release:        1%{?dist}
Summary:        Link Mod Manager for Bethesda games
License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/linkmm
BuildRequires:  cargo
BuildRequires:  rust >= 1.85
BuildRequires:  gcc-c++
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)

%description
linkmm is a graphical mod manager for Bethesda games.

%prep
# Nothing to do — built in-place with --build-in-place

%build
cargo build --release

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 io.github.sachesi.linkmm.desktop \
    %{buildroot}%{_datadir}/applications/io.github.sachesi.linkmm.desktop

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/io.github.sachesi.linkmm.desktop

%changelog
* Sat Mar 21 2026 linkmm packager
- Initial Fedora spec file
