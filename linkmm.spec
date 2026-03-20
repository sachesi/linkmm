%global crate linkmm

Name:           linkmm
Version:        0.1.0
Release:        1%{?dist}
Summary:        Link-based mod manager for Bethesda games

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/%{crate}
Source0:        %{url}/archive/v%{version}/%{crate}-%{version}.tar.gz
# To generate the vendor tarball:
#   spectool -g linkmm.spec
#   tar -xf linkmm-%%{version}.tar.gz
#   cd linkmm-%%{version}
#   cargo vendor
#   tar -czf linkmm-%%{version}-vendor.tar.gz vendor/
Source1:        %{crate}-%{version}-vendor.tar.gz

BuildRequires:  cargo-rpm-macros >= 24
BuildRequires:  pkgconfig(gtk4) >= 4.14
BuildRequires:  pkgconfig(libadwaita-1) >= 1.5
BuildRequires:  pkgconfig(glib-2.0) >= 2.80
BuildRequires:  pkgconfig(openssl)
BuildRequires:  desktop-file-utils

Requires:       gtk4
Requires:       libadwaita

%description
Link Mod Manager (linkmm) is a mod manager for Bethesda games on Linux.
It handles downloading, installing, and managing mods from Nexus Mods,
including support for the nxm:// URL scheme for one-click mod installation.

%prep
%autosetup -n %{crate}-%{version} -p1
%cargo_prep -v vendor

%build
%cargo_build

%install
%cargo_install

desktop-file-install \
    --dir %{buildroot}%{_datadir}/applications \
    io.github.sachesi.linkmm.desktop

%post
update-desktop-database %{_datadir}/applications &>/dev/null || :

%postun
update-desktop-database %{_datadir}/applications &>/dev/null || :

%check
%cargo_test

%files
%license LICENSE
%doc README.md
%{_bindir}/%{crate}
%{_datadir}/applications/io.github.sachesi.linkmm.desktop

%changelog
* Fri Mar 20 2026 Linkmm Maintainer <maintainer@example.com> - 0.1.0-1
- Initial package
