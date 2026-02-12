#!/bin/bash

set -e  # Exit on error

echo "=========================================="
echo "JobPlacer Installation"
echo "=========================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if Rust is installed
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}❌ Error: Rust is not installed${NC}"
    echo ""
    echo "Please install Rust from https://rustup.rs/"
    echo "Run this command:"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo ""
    exit 1
fi

echo -e "${GREEN}✓ Rust is installed${NC}"
RUST_VERSION=$(rustc --version)
echo "  $RUST_VERSION"

# Check if Python is installed
if ! command -v python3 &> /dev/null; then
    echo -e "${RED}❌ Error: Python 3 is not installed${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Python 3 is installed${NC}"
PYTHON_VERSION=$(python3 --version)
echo "  $PYTHON_VERSION"

# Check Python version (need 3.8+)
PYTHON_MINOR=$(python3 -c 'import sys; print(sys.version_info.minor)')
if [ "$PYTHON_MINOR" -lt 8 ]; then
    echo -e "${RED}❌ Error: Python 3.8+ is required (you have Python 3.$PYTHON_MINOR)${NC}"
    exit 1
fi

echo ""

# Get the directory where this script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

echo "Installation directory: $SCRIPT_DIR"
echo ""

# Step 1: Install Python build dependencies
echo "=========================================="
echo "Step 1: Installing Python dependencies"
echo "=========================================="
pip3 install --user -r "$SCRIPT_DIR/requirements.txt"
if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Python dependencies installed${NC}"
else
    echo -e "${RED}❌ Failed to install Python dependencies${NC}"
    exit 1
fi
echo ""

# Step 2: Build and install the Rust extension
echo "=========================================="
echo "Step 2: Building Rust Python extension"
echo "=========================================="
echo "This may take a few minutes..."
echo ""

cd "$SCRIPT_DIR"

# Use maturin to build and install
maturin develop --release
if [ $? -eq 0 ]; then
    echo ""
    echo -e "${GREEN}✓ Rust extension built and installed${NC}"
else
    echo -e "${RED}❌ Failed to build Rust extension${NC}"
    exit 1
fi
echo ""

# Step 3: Copy Python wrapper to site-packages or make it importable
echo "=========================================="
echo "Step 3: Setting up Python wrapper"
echo "=========================================="

PYTHON_WRAPPER="$SCRIPT_DIR/python/nodelists_generator_rust.py"
if [ -f "$PYTHON_WRAPPER" ]; then
    # Option 1: Add symlink or copy to site-packages
    # For simplicity, we'll just note the path
    echo -e "${YELLOW}Note: Python wrapper is at:${NC}"
    echo "  $PYTHON_WRAPPER"
    echo ""
    echo "To use it, either:"
    echo "  1. Add to PYTHONPATH: export PYTHONPATH=\"$SCRIPT_DIR/python:\$PYTHONPATH\""
    echo "  2. Install in development mode (recommended)"
fi
echo ""

# Step 4: Verify installation
echo "=========================================="
echo "Step 4: Verifying installation"
echo "=========================================="

python3 << EOF
import sys
try:
    import job_placer
    print("${GREEN}✓ job_placer module imported successfully${NC}")
    
    # Check if we can create a QueryBuilder
    qb = job_placer.TopologyQueryBuilder
    print("${GREEN}✓ TopologyQueryBuilder class available${NC}")
    
except ImportError as e:
    print("${RED}❌ Failed to import job_placer module${NC}")
    print(f"Error: {e}")
    sys.exit(1)
except Exception as e:
    print("${RED}❌ Error during verification${NC}")
    print(f"Error: {e}")
    sys.exit(1)
EOF

if [ $? -ne 0 ]; then
    exit 1
fi

echo ""
echo "=========================================="
echo -e "${GREEN}✓ Installation completed successfully!${NC}"
echo "=========================================="
echo ""
echo "JobPlacer is now installed and ready to use!"
echo ""
echo "Quick test:"
echo "  cd $SCRIPT_DIR"
echo "  python3 -c 'import job_placer; print(\"JobPlacer version:\", job_placer.__name__)'"
echo ""
echo "Next steps:"
echo "  1. Make sure you have a Leonardo topology file (leo.txt)"
echo "  2. See examples in the README.md"
echo "  3. Run tests: python3 python/test_leonardo.py"
echo ""