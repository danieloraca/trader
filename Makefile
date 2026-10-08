.PHONY: build deploy status logs

SERVICE := trader.service
INSTALL_DIR := /opt/trader
DEPLOY_TARGET_DIR := target/deploy

build:
	cargo build --release --bin trader

# Run from the checkout on the Pi to update its existing trader service.
deploy:
	@test "$$(uname -s)" = "Linux" || { echo "Run make deploy on the Raspberry Pi" >&2; exit 1; }
	@set -eu; \
		exec_start="$$(systemctl show --property=ExecStart --value $(SERVICE))"; \
		case "$$exec_start" in \
			*"path=$(CURDIR)/target/release/trader "*) \
				cargo build --release --bin trader --target-dir $(DEPLOY_TARGET_DIR); \
				install -m 0755 $(DEPLOY_TARGET_DIR)/release/trader target/release/.trader.new; \
				mv -f target/release/.trader.new target/release/trader ;; \
			*"path=$(INSTALL_DIR)/trader "*) \
				cargo build --release --bin trader; \
				sudo install -o root -g root -m 0755 target/release/trader $(INSTALL_DIR)/.trader.new; \
				sudo mv -f $(INSTALL_DIR)/.trader.new $(INSTALL_DIR)/trader ;; \
			*) echo "Unsupported $(SERVICE) ExecStart: $$exec_start" >&2; exit 1 ;; \
		esac
	sudo systemctl restart $(SERVICE)
	@sleep 2
	@systemctl is-active --quiet $(SERVICE) || { systemctl --no-pager --full status $(SERVICE); exit 1; }
	@systemctl is-active $(SERVICE)

status:
	systemctl --no-pager --full status $(SERVICE)

logs:
	journalctl -u $(SERVICE) -f
