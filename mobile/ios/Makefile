# The iPhone app (see AGENTS.md in this folder). Run from mobile/ios.
#
#   make open        generate the Xcode project and open it (then Run in Xcode)
#   make project     generate UtopiaMobile.xcodeproj from project.yml
#   make bindings    build the Rust core for the Mac and write the Swift bindings
#   make sim         build for the iOS Simulator with xcodebuild
#   make device      build for an iPhone with xcodebuild (automatic signing)
#   make install DEVICE=<udid>   build for an iPhone and install it with devicectl
#   make check       the scripted check: bindings, Swift check, iOS type-check (xtask mobile)
#   make clean       remove the generated project, bindings and Xcode build output

ROOT        := $(abspath ../..)
PROJECT     := UtopiaMobile.xcodeproj
SCHEME      := UtopiaMobile
DERIVED     := $(ROOT)/target/xcode/DerivedData
SIM         ?= iPhone 18 Pro
CONFIG      ?= Debug
DEVICE      ?=
XCODEBUILD  := xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration $(CONFIG) \
               -derivedDataPath $(DERIVED)

.PHONY: help open project bindings sim device install check clean

help:
	@sed -n '3,10p' Makefile | sed 's/^# //'

bindings:
	./scripts/build-core.sh

project: bindings
	xcodegen generate --spec project.yml

open: project
	open $(PROJECT)

sim: project
	$(XCODEBUILD) -destination 'platform=iOS Simulator,name=$(SIM)' build

device: project
	$(XCODEBUILD) -destination 'generic/platform=iOS' -allowProvisioningUpdates build

install: project
	@test -n "$(DEVICE)" || { echo "usage: make install DEVICE=<udid> (xcrun devicectl list devices)"; exit 2; }
	$(XCODEBUILD) -destination 'id=$(DEVICE)' -allowProvisioningUpdates build
	xcrun devicectl device install app --device $(DEVICE) \
	    "$(DERIVED)/Build/Products/$(CONFIG)-iphoneos/UtopiaMobile.app"

check:
	cd $(ROOT) && cargo run -p xtask -- mobile

clean:
	rm -rf $(PROJECT) Generated $(ROOT)/target/xcode
