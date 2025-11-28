.PHONY: help build build-plugin test test-all examples example-simple example-streaming clean all

# Default target
.DEFAULT_GOAL := help

# Colors for output
BLUE := \033[0;34m
GREEN := \033[0;32m
YELLOW := \033[0;33m
NC := \033[0m # No Color

# Detect OS for plugin library extension
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Linux)
	PLUGIN_EXT := .so
	PLUGIN_PREFIX := lib
endif
ifeq ($(UNAME_S),Darwin)
	PLUGIN_EXT := .dylib
	PLUGIN_PREFIX := lib
endif
ifeq ($(OS),Windows_NT)
	PLUGIN_EXT := .dll
	PLUGIN_PREFIX :=
endif

PLUGIN_NAME := $(PLUGIN_PREFIX)nylon_ring_plugin_example$(PLUGIN_EXT)
PLUGIN_PATH := target/debug/$(PLUGIN_NAME)

help: ## แสดง help message
	@echo "$(BLUE)Nylon Ring - Makefile Commands$(NC)"
	@echo ""
	@echo "$(GREEN)Build Commands:$(NC)"
	@echo "  $(YELLOW)make build$(NC)              - Build all crates"
	@echo "  $(YELLOW)make build-plugin$(NC)        - Build plugin library (cdylib)"
	@echo "  $(YELLOW)make all$(NC)                 - Build everything (default: debug)"
	@echo ""
	@echo "$(GREEN)Test Commands:$(NC)"
	@echo "  $(YELLOW)make test$(NC)                - Run all tests"
	@echo "  $(YELLOW)make test-all$(NC)            - Run all tests with verbose output"
	@echo ""
	@echo "$(GREEN)Example Commands:$(NC)"
	@echo "  $(YELLOW)make examples$(NC)             - Run all examples"
	@echo "  $(YELLOW)make example-simple$(NC)       - Run simple_host example"
	@echo "  $(YELLOW)make example-streaming$(NC)   - Run streaming_host example"
	@echo ""
	@echo "$(GREEN)Utility Commands:$(NC)"
	@echo "  $(YELLOW)make clean$(NC)               - Clean build artifacts"
	@echo "  $(YELLOW)make check-plugin$(NC)        - Check if plugin exists"
	@echo ""

build: ## Build all crates
	@echo "$(BLUE)Building all crates...$(NC)"
	@cargo build
	@echo "$(GREEN)✓ Build complete!$(NC)"

build-plugin: ## Build plugin library (cdylib)
	@echo "$(BLUE)Building plugin library...$(NC)"
	@cargo build -p nylon-ring-plugin-example
	@if [ -f "$(PLUGIN_PATH)" ]; then \
		echo "$(GREEN)✓ Plugin built: $(PLUGIN_PATH)$(NC)"; \
	else \
		echo "$(YELLOW)⚠ Plugin not found at expected path: $(PLUGIN_PATH)$(NC)"; \
	fi

all: build build-plugin ## Build everything (all crates + plugin)
	@echo "$(GREEN)✓ All builds complete!$(NC)"

test: ## Run all tests
	@echo "$(BLUE)Running tests...$(NC)"
	@cargo test --workspace --lib
	@echo "$(GREEN)✓ Tests complete!$(NC)"

test-all: ## Run all tests with verbose output
	@echo "$(BLUE)Running tests (verbose)...$(NC)"
	@cargo test --workspace --lib -- --nocapture
	@echo "$(GREEN)✓ Tests complete!$(NC)"

check-plugin: ## Check if plugin library exists
	@if [ -f "$(PLUGIN_PATH)" ]; then \
		echo "$(GREEN)✓ Plugin found: $(PLUGIN_PATH)$(NC)"; \
		ls -lh "$(PLUGIN_PATH)"; \
	else \
		echo "$(YELLOW)⚠ Plugin not found: $(PLUGIN_PATH)$(NC)"; \
		echo "$(YELLOW)Run 'make build-plugin' first$(NC)"; \
		exit 1; \
	fi

example-simple: check-plugin ## Run simple_host example
	@echo "$(BLUE)Running simple_host example...$(NC)"
	@cargo run --example simple_host
	@echo "$(GREEN)✓ Example complete!$(NC)"

example-streaming: check-plugin ## Run streaming_host example
	@echo "$(BLUE)Running streaming_host example...$(NC)"
	@cargo run --example streaming_host
	@echo "$(GREEN)✓ Example complete!$(NC)"

examples: check-plugin ## Run all examples
	@echo "$(BLUE)Running all examples...$(NC)"
	@echo ""
	@echo "$(YELLOW)=== Example 1: simple_host ===$(NC)"
	@cargo run --example simple_host
	@echo ""
	@echo "$(YELLOW)=== Example 2: streaming_host ===$(NC)"
	@cargo run --example streaming_host
	@echo ""
	@echo "$(GREEN)✓ All examples complete!$(NC)"

clean: ## Clean build artifacts
	@echo "$(BLUE)Cleaning build artifacts...$(NC)"
	@cargo clean
	@echo "$(GREEN)✓ Clean complete!$(NC)"

