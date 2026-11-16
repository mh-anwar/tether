import os
import subprocess
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
            subprocess.run([str(Path(os.environ['TETHER_APP']).with_name('tether')), 'accept'], check=True)
            break
        if phone_log.exists() and 'connected to' in phone_log.read_text(): break
        time.sleep(.1)
    w.until(lambda _: d.find_elements(By.CSS_SELECTOR, '.received-panel'))
    assert 'Files you receive appear here' in d.find_element(By.CSS_SELECTOR,'.received-panel').text
    print('ok received files empty state')
    d.find_element(By.XPATH,"//button[.//span[text()='Settings']]").click()
    w.until(lambda _: d.find_elements(By.CSS_SELECTOR,'[data-key=set-send_notifications]'))
    switch = d.find_element(By.CSS_SELECTOR,'[data-key=set-send_notifications]')
    assert not switch.is_selected()
    switch.click()
    w.until(lambda _: d.find_element(By.CSS_SELECTOR,'[data-key=set-send_notifications]').is_selected())
    print('ok desktop notification switch defaults off and saves')
    assert d.find_element(By.CSS_SELECTOR,'select[aria-label="Pause media during calls"]').get_attribute('value') == 'ringing'
    print('ok call media settings')
    d.save_screenshot('/tmp/tw-media/settings.png')
    d.find_element(By.TAG_NAME,'body').send_keys(Keys.CONTROL,'k')
    inp = w.until(lambda _: d.find_element(By.CSS_SELECTOR,'.palette input'))
    inp.send_keys('Send text or link')
    w.until(lambda _: d.find_element(By.XPATH,"//div[contains(@class,'palette')]//button[contains(.,'Send text or link')]" )).click()
    w.until(lambda _: d.find_element(By.CSS_SELECTOR,'textarea[aria-label="Text or link"]'))
    print('ok send text or link palette form')
finally:
    d.quit()
