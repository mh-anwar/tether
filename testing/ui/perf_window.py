"""Times the window with a big phone: 500 conversations, 1500 contacts. Run: testing/ui/wd.sh testing/ui/perf_window.py

Fills the window's own state (no phone needs that much data) and reports how long each update takes,
layout included. Fails when one incoming text freezes the window for longer than a frame budget of 50 ms.
"""

import os
import sys

from selenium import webdriver
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support.ui import WebDriverWait

opts = ArgOptions()
opts.set_capability("browserName", "wry")
opts.set_capability("tauri:options", {"application": os.environ["TETHER_APP"]})
d = webdriver.Remote(command_executor=os.environ["WD_URL"], options=opts)
BUDGET_MS = 50
try:
    WebDriverWait(d, 20).until(lambda _: d.execute_script("return !!document.querySelector('.threads > *')"))
    r = d.execute_script("""
      const now = Date.now();
      state.contacts = Array.from({length: 1500}, (_, i) => ({name: `Person ${i}`, number: String(15550000000 + i)}));
      const msgs = [];
      for (let t = 1000; t < 1500; t++) for (let k = 0; k < 20; k++)
        msgs.push({_id: t * 100 + k, thread_id: t, body: `Message ${k} in conversation ${t}`, date: now - t * 60000 - k * 1000,
          type: k % 2 ? 2 : 1, read: 1, addresses: [{address: `+1555000${String(t).padStart(4, '0')}`}], attachments: []});
      mergeMessages(msgs);
      // Layout is part of what the user waits for, so force it inside the timer.
      const time = (fn) => { const t0 = performance.now(); fn(); flushRender(); document.body.offsetHeight; return Math.round((performance.now() - t0) * 10) / 10; };
      const incoming = (n) => ({event: 'update', device_id: state.deviceId, update: {kind: 'messages', messages: [
        {_id: 9000000 + n, thread_id: 1000 + n, body: 'New text', date: Date.now(), type: 1, read: 0,
         addresses: [{address: `+1555000${String(1000 + n).padStart(4, '0')}`}], attachments: []}]}});
      const out = {};
      out['conversation list, first draw'] = time(renderList);
      out['conversation list, redraw'] = time(renderList);
      out['side rail'] = time(renderRail);
      out['one incoming text'] = time(() => onEvent(incoming(1)));
      out['20 texts arriving together'] = time(() => { for (let i = 2; i < 22; i++) onEvent(incoming(i)); });
      out['open a conversation'] = time(() => selectThread(1100));
      out['rows drawn'] = document.querySelectorAll('.thread-item').length;
      const box = document.querySelector('.threads');
      box.scrollTop = box.scrollHeight; box.dispatchEvent(new Event('scroll'));
      out['rows drawn after scrolling to the end'] = document.querySelectorAll('.thread-item').length;
      return out;
    """)
finally:
    d.quit()
for k, v in r.items():
    print(f"{v:>8}  {k}")
slow = r["one incoming text"] > BUDGET_MS
print(f"FAILED: one incoming text took {r['one incoming text']} ms (budget {BUDGET_MS})" if slow else "PASSED")
sys.exit(1 if slow else 0)
