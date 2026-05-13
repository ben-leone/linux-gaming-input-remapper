APP_NAME          := gameremap
BINARY_NAME       := gameremap
ARCH              := x86_64
APP_VERSION       ?= dev
BUILD_DIR         := $(shell pwd)/build
DIST_DIR          := $(shell pwd)/dist
APPIMAGETOOL      := $(BUILD_DIR)/appimagetool-x86_64.AppImage
DOCKER_IMAGE      := gameremap-builder
DOCKER_CARGO_CACHE := gameremap-cargo-cache

APPIMAGE_OUT      := $(BUILD_DIR)/$(APP_NAME)-$(APP_VERSION)-$(ARCH).AppImage

.PHONY: help build build-release test clean deps \
        package docker-image docker-build-release \
        docker-build-appimage docker-clean run-debug run

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-24s\033[0m %s\n", $$1, $$2}'

# ── Native build ─────────────────────────────────────────────────────

build: ## Build debug binary (native)
	cargo build

build-release: ## Build release binary (native, portable x86-64)
	RUSTFLAGS="-C target-cpu=x86-64" cargo build --release

test: ## Run tests
	cargo test

clean: ## Clean build artifacts
	cargo clean
	rm -rf $(BUILD_DIR)

run-debug: ## Run key monitor debug window
	cargo run -- debug

run: ## Run the AppImage profile editor (requires 'make package' first)
	$(APPIMAGE_OUT)

# ── AppImage packaging (binary must already exist) ───────────────────

deps: ## Download appimagetool if needed
	@mkdir -p $(BUILD_DIR)
	@if [ ! -f $(APPIMAGETOOL) ]; then \
		echo "Downloading appimagetool..."; \
		curl -L -o $(APPIMAGETOOL) \
			"https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"; \
		chmod +x $(APPIMAGETOOL); \
		echo "Done."; \
	else \
		echo "appimagetool already present."; \
	fi

package: deps ## Package AppImage from existing release binary
	@test -f target/release/$(BINARY_NAME) || \
		{ echo "ERROR: run 'make build-release' or 'make docker-build-release' first"; exit 1; }
	@rm -rf $(BUILD_DIR)/$(APP_NAME).AppDir
	@mkdir -p $(BUILD_DIR)/$(APP_NAME).AppDir/usr/bin
	@mkdir -p $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/applications
	@mkdir -p $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/icons/hicolor/256x256/apps
	@mkdir -p $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/$(APP_NAME)

	cp target/release/$(BINARY_NAME)                        $(BUILD_DIR)/$(APP_NAME).AppDir/usr/bin/
	cp packaging/appimage/AppRun                            $(BUILD_DIR)/$(APP_NAME).AppDir/AppRun
	chmod +x                                                $(BUILD_DIR)/$(APP_NAME).AppDir/AppRun
	cp packaging/appimage/$(APP_NAME).desktop               $(BUILD_DIR)/$(APP_NAME).AppDir/
	cp packaging/appimage/$(APP_NAME).desktop               $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/applications/
	cp packaging/appimage/icon.png                          $(BUILD_DIR)/$(APP_NAME).AppDir/$(APP_NAME).png
	cp packaging/appimage/icon.png                          $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/icons/hicolor/256x256/apps/$(APP_NAME).png
	cp -r devices/                                          $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/$(APP_NAME)/
	cp -r packaging/udev/                                   $(BUILD_DIR)/$(APP_NAME).AppDir/usr/share/$(APP_NAME)/

	ARCH=$(ARCH) APPIMAGE_EXTRACT_AND_RUN=1 $(APPIMAGETOOL) \
		$(BUILD_DIR)/$(APP_NAME).AppDir \
		$(APPIMAGE_OUT)
	@echo ""
	@echo "AppImage: $(APPIMAGE_OUT)"
	@ls -lh $(APPIMAGE_OUT)

# ── Docker targets ───────────────────────────────────────────────────

docker-image: ## Build the Docker builder image
	docker build -f Dockerfile.build -t $(DOCKER_IMAGE) .

docker-build-release: docker-image ## Compile release binary inside Docker
	docker run --rm \
		-v $(shell pwd):/src \
		-v $(DOCKER_CARGO_CACHE):/usr/local/cargo/registry \
		$(DOCKER_IMAGE) \
		bash -c 'RUSTFLAGS="-C target-cpu=x86-64" cargo build --release'

docker-build-appimage: docker-build-release package ## Docker compile + package AppImage
	@mkdir -p $(DIST_DIR)
	cp $(APPIMAGE_OUT) $(DIST_DIR)/
	@echo "Artifact: $(DIST_DIR)/$(notdir $(APPIMAGE_OUT))"
	@ls -lh $(DIST_DIR)/$(notdir $(APPIMAGE_OUT))

docker-clean: ## Remove Docker builder image and cargo cache volume
	-docker rmi $(DOCKER_IMAGE)
	-docker volume rm $(DOCKER_CARGO_CACHE)
