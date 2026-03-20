NAME    = linkmm
VERSION = $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
GIT_SSH = git@github.com:sachesi/$(NAME).git
TAG     = v$(VERSION)

SOURCES_DIR = $(HOME)/rpmbuild/SOURCES
WORK_DIR    = /tmp/$(NAME)-$(VERSION)-rpm-work

.PHONY: sources

# Generate both source tarballs needed for rpmbuild.
#
# Prerequisites:
#   - SSH access to the private repository configured in ~/.ssh/config
#   - git, cargo, tar, gzip installed
#
# Usage:
#   make sources           # uses version from Cargo.toml
#   make sources VERSION=0.2.0
sources: $(SOURCES_DIR)/$(NAME)-$(VERSION).tar.gz \
         $(SOURCES_DIR)/$(NAME)-$(VERSION)-vendor.tar.gz

$(SOURCES_DIR)/$(NAME)-$(VERSION).tar.gz:
	@mkdir -p $(SOURCES_DIR)
	git archive --remote=$(GIT_SSH) $(TAG) \
	    --prefix=$(NAME)-$(VERSION)/ \
	    | gzip > $@
	@echo "Created $@"

$(SOURCES_DIR)/$(NAME)-$(VERSION)-vendor.tar.gz: \
        $(SOURCES_DIR)/$(NAME)-$(VERSION).tar.gz
	rm -rf $(WORK_DIR)
	mkdir -p $(WORK_DIR)
	tar -xf $< -C $(WORK_DIR)
	cd $(WORK_DIR)/$(NAME)-$(VERSION) && cargo vendor
	tar -czf $@ -C $(WORK_DIR)/$(NAME)-$(VERSION) vendor/
	rm -rf $(WORK_DIR)
	@echo "Created $@"
