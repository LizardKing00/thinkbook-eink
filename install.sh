#!/bin/bash

set -e

echo "Building thinkbook-eink..."
cargo build --release

echo "Installing binaries to /usr/local/bin/..."
sudo cp target/release/setbackside /usr/local/bin/setbackside
sudo cp target/release/eink-clock /usr/local/bin/eink-clock
sudo cp target/release/eink-info /usr/local/bin/eink-info
sudo cp target/release/eink-server /usr/local/bin/eink-server

echo "Installing udev rule..."
sudo cp udev/99-thinkbook-eink.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger

echo "Adding user to plugdev group..."
sudo usermod -aG plugdev "$USER"

echo ""
echo "Installation complete."
echo ""
echo "IMPORTANT: Log out and back in for the plugdev group change to take effect."
echo "After that, you can run setbackside, eink-clock and eink-info without sudo."
echo ""
echo "Usage:"
echo "  setbackside ~/path/to/image.jpg"
echo "  eink-clock"
echo "  eink-info"
echo "  eink-server"
