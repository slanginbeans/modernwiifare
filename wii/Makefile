# Builds the Phase 0 spike .dol files.
#   Needs devkitPro (devkitPPC + libogc) and the Rust toolchain from rust-toolchain.toml.
#   make            -> build/spike_a_gx.dol, build/spike_b_net.dol
#   make hbc        -> build/hbc/apps/<spike>/{boot.dol,meta.xml}  (copy to SD:/apps/)
#   make run-a|run-b WIILOAD=tcp:<wii-ip>   -> send over the network with wiiload

ifeq ($(strip $(DEVKITPRO)),)
$(error DEVKITPRO is not set. Install devkitPro (wii-dev) or use the devkitpro/devkitppc container)
endif

DEVKITPPC ?= $(DEVKITPRO)/devkitPPC
CC        := $(DEVKITPPC)/bin/powerpc-eabi-gcc
ELF2DOL   := $(DEVKITPRO)/tools/bin/elf2dol
WIILOAD_BIN := $(DEVKITPRO)/tools/bin/wiiload
OGC       := $(DEVKITPRO)/libogc

MACHDEP := -DGEKKO -mrvl -mcpu=750 -meabi -mhard-float
CFLAGS  := -O2 -g -Wall -Wextra -ffunction-sections -fdata-sections $(MACHDEP) -I$(OGC)/include -Iplatform
LDFLAGS := -g $(MACHDEP) -L$(OGC)/lib/wii -Wl,--gc-sections
LIBS    := -logc -lm

RUST_TARGET := powerpc-unknown-linux-gnu
RUSTLIB     := ../target/$(RUST_TARGET)/release/libmwf_spikes.a
BUILD_ID    ?= $(shell git rev-parse --short=8 HEAD 2>/dev/null || echo dev)

BUILD  := build
COMMON := $(BUILD)/shim_common.o $(BUILD)/shim_gx.o $(BUILD)/shim_net.o
SPIKES := spike_a_gx spike_b_net

.PHONY: all hbc clean rustlib run-a run-b
all: $(SPIKES:%=$(BUILD)/%.dol)

# Cargo tracks its own dependencies; always ask it.
rustlib:
	MWF_BUILD_ID=$(BUILD_ID) cargo build --release -p mwf_spikes --target $(RUST_TARGET)
$(RUSTLIB): rustlib

$(BUILD):
	mkdir -p $@

$(BUILD)/%.o: platform/%.c platform/shim.h | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/spike_a_gx.elf: $(BUILD)/main_spike_a.o $(COMMON) $(RUSTLIB)
	$(CC) $(LDFLAGS) $(BUILD)/main_spike_a.o $(COMMON) $(RUSTLIB) $(LIBS) -Wl,-Map,$@.map -o $@

$(BUILD)/spike_b_net.elf: $(BUILD)/main_spike_b.o $(COMMON) $(RUSTLIB)
	$(CC) $(LDFLAGS) $(BUILD)/main_spike_b.o $(COMMON) $(RUSTLIB) $(LIBS) -Wl,-Map,$@.map -o $@

$(BUILD)/%.dol: $(BUILD)/%.elf
	$(ELF2DOL) $< $@

hbc: all
	@for s in $(SPIKES); do \
	  mkdir -p $(BUILD)/hbc/apps/mwf_$$s; \
	  cp $(BUILD)/$$s.dol $(BUILD)/hbc/apps/mwf_$$s/boot.dol; \
	  sed "s/@BUILD_ID@/$(BUILD_ID)/" hbc/$$s/meta.xml > $(BUILD)/hbc/apps/mwf_$$s/meta.xml; \
	done
	@echo "Copy $(BUILD)/hbc/apps/* to SD:/apps/"

run-a: $(BUILD)/spike_a_gx.dol
	$(WIILOAD_BIN) $<
run-b: $(BUILD)/spike_b_net.dol
	$(WIILOAD_BIN) $<

clean:
	rm -rf $(BUILD)
