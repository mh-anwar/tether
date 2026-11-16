import json
import os
import subprocess
import sys
import time
from pathlib import Path
from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support.ui import WebDriverWait
opts = ArgOptions()
opts.set_capability('browserName', 'wry')
opts.set_capability('tauri:options', {'application': os.environ['TETHER_APP']})
d = webdriver.Remote(command_executor=os.environ['WD_URL'], options=opts)
w = WebDriverWait(d, 25)
try:
    # Pair through the real CLI in the isolated test environment, if this is its first run.
    for _ in range(60):
        phone_log = Path(os.environ['XDG_DATA_HOME']).parent / 'phone.log'
        if phone_log.exists() and 'asked to pair' in phone_log.read_text():
            # The daemon may not have registered the request yet.
            for _ in range(30):
                if subprocess.run([str(Path(os.environ['TETHER_APP']).with_name('tether')), 'accept'], capture_output=True).returncode == 0: break
                time.sleep(.2)
            break
        if phone_log.exists() and 'connected to' in phone_log.read_text(): break
        time.sleep(.1)
    w.until(lambda _: d.find_elements(By.CSS_SELECTOR, '.received-panel'))
    assert 'Files you receive appear here' in d.find_element(By.CSS_SELECTOR,'.received-panel').text
    print('ok received files empty state')
    # Phone shares two files and a third is mid-transfer, as the window shows it.
    control = Path(os.environ['TETHER_UI_DIR']) / 'control'
    src = control.parent / 'photo-src'
    src.write_bytes(b'x' * 4096)
    for i, name in enumerate(['holiday.jpg', 'notes.txt']):
        (control / f'{i}.tmp').write_text(json.dumps({'id': 900 + i, 'type': 'kdeconnect.share.request', 'testPayloadPath': str(src),
            'body': {'filename': name, 'numberOfFiles': 3, 'totalPayloadSize': 8192 + 1000000, 'open': False}}))
        (control / f'{i}.tmp').rename(control / f'{i:04}.json')
    w.until(lambda _: len(d.find_elements(By.XPATH, "//section[contains(@class,'received-panel')]//button[text()='Show in folder']")) == 2)
    assert (Path(os.environ['TETHER_UI_DIR']) / 'dl' / 'holiday.jpg').exists()
    print('ok received files listed with Open and Show in folder')
    d.execute_script("transferProgress({name:'video from phone.mp4',sending:false,bytes:640000,total:1000000,number_of_files:3,total_payload_size:1008192})")
    w.until(lambda _: d.find_elements(By.CSS_SELECTOR, '.received-panel progress'))
    assert 'Receiving' in d.find_element(By.CSS_SELECTOR, '.received-panel').text
    print('ok receiving progress shown')
    if os.environ.get('SHOT_OUT'):
        subprocess.run([sys.executable, str(Path(__file__).parents[1] / 'ui/shot.py'), os.environ['DISPLAY'], os.environ['SHOT_OUT']], check=True)
    d.find_element(By.XPATH,"//button[.//span[text()='Settings']]").click()
    w.until(lambda _: d.find_elements(By.CSS_SELECTOR,'[data-key=set-send_notifications]'))
    switch = d.find_element(By.CSS_SELECTOR,'[data-key=set-send_notifications]')
    assert not switch.is_selected()
    switch.click()
    w.until(lambda _: d.find_element(By.CSS_SELECTOR,'[data-key=set-send_notifications]').is_selected())
    print('ok desktop notification switch defaults off and saves')
    assert d.find_element(By.CSS_SELECTOR,'select[aria-label="Pause media during calls"]').get_attribute('value') == 'ringing'
    print('ok call media settings')
    d.save_screenshot(str(Path(os.environ['TETHER_UI_DIR']) / 'settings.png'))
    d.find_element(By.TAG_NAME,'body').send_keys(Keys.CONTROL,'k')
    inp = w.until(lambda _: d.find_element(By.CSS_SELECTOR,'.palette input'))
    inp.send_keys('Send text or link')
    w.until(lambda _: d.find_elements(By.XPATH, "//div[contains(@class,'palette')]//li[contains(.,'Send text or link')]"))
    inp.send_keys(Keys.ENTER)
    box = w.until(lambda _: d.find_element(By.CSS_SELECTOR, 'textarea[aria-label="Text or link"]'))
    print('ok send text or link palette form')
    received = Path(os.environ['TETHER_UI_DIR']) / 'control' / 'received.jsonl'
    def got(body_key, value):
        return received.exists() and any(json.loads(l).get('type') == 'kdeconnect.share.request' and json.loads(l)['body'].get(body_key) == value
                                         for l in received.read_text().splitlines())
    box.send_keys('https://example.org/from-desktop')
    d.find_element(By.XPATH, "//form[@role='dialog']//button[text()='Send']").click()
    w.until(lambda _: got('url', 'https://example.org/from-desktop'))
    print('ok link reaches the phone as a url share')
    d.find_element(By.TAG_NAME, 'body').send_keys(Keys.CONTROL, 'k')
    inp = w.until(lambda _: d.find_element(By.CSS_SELECTOR, '.palette input'))
    inp.send_keys('Send text or link')
    w.until(lambda _: d.find_elements(By.XPATH, "//div[contains(@class,'palette')]//li[contains(.,'Send text or link')]"))
    inp.send_keys(Keys.ENTER)
    box = w.until(lambda _: d.find_element(By.CSS_SELECTOR, 'textarea[aria-label="Text or link"]'))
    box.send_keys('see you at 6')
    d.find_element(By.XPATH, "//form[@role='dialog']//button[text()='Send']").click()
    w.until(lambda _: got('text', 'see you at 6'))
    print('ok text reaches the phone as a text share')
finally:
    d.quit()
