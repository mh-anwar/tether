# Saves a screenshot of a virtual X display. Run: uv run --no-project --with pillow python testing/ui/shot.py :57 out.png
import sys
from PIL import ImageGrab
im = ImageGrab.grab(xdisplay=sys.argv[1])
im.save(sys.argv[2]); print(im.size)
