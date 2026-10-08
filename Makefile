.PHONY: build deploy status logs

SERVICE := trader.service
INSTALL_DIR := /opt/trader

build:
	cargo build --release --bin trader

# Run from the checkout on the Pi to update its existing trader service.
deploy:
	@test "$$(uname -s)" = "Linux" || { echo "Run make deploy on the Raspberry Pi" >&2; exit 1; }
	@systemctl show --property=ExecStart --value $(SERVICE) | grep -Fq 'path=$(INSTALL_DIR)/trader ' || { echo "Expected $(SERVICE) to run $(INSTALL_DIR)/trader; check systemctl cat $(SERVICE)" >&2; exit 1; }
	cargo build --release --bin trader
	sudo install -o root -g root -m 0755 target/release/trader $(INSTALL_DIR)/.trader.new
	sudo mv -f $(INSTALL_DIR)/.trader.new $(INSTALL_DIR)/trader
	sudo systemctl restart $(SERVICE)
	@sleep 2
	@systemctl is-active --quiet $(SERVICE) || { systemctl --no-pager --full status $(SERVICE); exit 1; }
	@systemctl is-active $(SERVICE)

status:
	systemctl --no-pager --full status $(SERVICE)

logs:
	journalctl -u $(SERVICE) -f
