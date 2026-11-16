"""Window smoke test over WebDriver. Run with testing/ui/wd.sh (it starts everything this needs).

Finds things by role and text, the way a person would, so layout changes don't break it.
"""

import os
import sys
from pathlib import Path

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait

opts = ArgOptions()
opts.set_capability("browserName", "wry")
opts.set_capability("tauri:options", {"application": os.environ["TETHER_APP"]})
d = webdriver.Remote(command_executor=os.environ["WD_URL"], options=opts)
wait = WebDriverWait(d, 20)
failures = []


def check(name, fn):
    try:
        fn()
        print(f"ok    {name}")
    except Exception as e:  # report every step, not just the first failure
        failures.append(name)
        print(f"FAIL  {name}: {type(e).__name__} {str(e).splitlines()[0] if str(e) else ''}")


def button(text):
    return wait.until(EC.element_to_be_clickable(
        (By.XPATH, f"//button[normalize-space(.)='{text}' or .//span[normalize-space(.)='{text}']]")))


def text_shown(text):
    # The element whose own text holds it, so hidden text elsewhere on the page doesn't count.
    wait.until(EC.visibility_of_element_located(
        (By.XPATH, f"//*[text()[contains(normalize-space(.), \"{text}\")]]")))


try:
    check("window connects to the phone", lambda: text_shown("Connected"))
    check("conversations load", lambda: wait.until(lambda _: d.find_elements(By.CSS_SELECTOR, ".threads > *")))

    def copy_screenshot():
        if not os.environ.get("FAKE_SFTP"):
            raise RuntimeError("no stand-in file server (FAKE_SFTP); run wd.sh once more")
        card = wait.until(EC.visibility_of_element_located(
            (By.CSS_SELECTOR, "article.note-card[aria-label*='Screenshot saved']")))
        card.find_element(By.XPATH, ".//button[normalize-space(.)='Copy']").click()
        text_shown("Screenshot copied")
    check("screenshot notification copies the picture", copy_screenshot)

    def photos_page():
        button("Photos").click()
        wait.until(lambda _: len(d.find_elements(By.CSS_SELECTOR, ".photo-tile img")) == 5)
        d.find_element(By.XPATH, "//button[@role='tab'][contains(., 'Screenshots')]").click()
        wait.until(lambda _: len(d.find_elements(By.CSS_SELECTOR, ".photo-tile img")) == 2)
        d.find_element(By.CSS_SELECTOR, ".photo-tile").click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".viewer-body img")))
        button("Save to Downloads").click()
        text_shown("Saved to")
        d.find_element(By.CSS_SELECTOR, ".viewer button[aria-label='Close']").click()
    check("Photos page shows the phone's pictures and saves one", photos_page)

    def apps_page():
        button("Apps").click()
        text_shown("phone's apps")
    check("Apps page opens", apps_page)

    def code_form():
        Path(os.environ["FAKE_APPS_DIR"], "codeopen").touch()
        button("Pair with a code instead").click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=apps-code]")))
        text_shown("Wi-Fi pairing code")
    check("code pairing form shows", code_form)

    def pair_for_apps():
        wait.until(lambda _: d.execute_script("return apps.status?.pairingAddr"))
        d.find_element(By.CSS_SELECTOR, "[data-key=apps-code]").send_keys("123456")
        button("Pair").click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".app-tile")))
    check("pairing reaches the app grid", pair_for_apps)

    def reconnect_without_announcements():
        fake = Path(os.environ["FAKE_APPS_DIR"])
        # The connection port is remembered only after adb reports the phone online.
        saved = Path(os.environ["XDG_DATA_HOME"], "tether/apps-connect-address")
        wait.until(lambda _: saved.exists())
        (fake / "hideconnect").touch()
        (fake / "connected").unlink()
        # Poll now rather than waiting for the app grid's ten-second interval.
        d.execute_script("pollApps()")
        wait.until(lambda _: (fake / "connected").exists())
        wait.until(lambda _: d.execute_script("return !!apps.status?.device"))
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".app-tile")))
    check("paired phone reconnects without mDNS announcements", reconnect_without_announcements)

    def palette():
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "k")
        wait.until(EC.visibility_of_element_located((By.XPATH, "//*[normalize-space(.)='Check for updates']")))
        d.switch_to.active_element.send_keys(Keys.ESCAPE)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, ".palette-backdrop"))
    check("Ctrl+K offers Check for updates", palette)

    def settings_page():
        button("Settings").click()
        box = wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, "[data-key=set-auto_update]")))
        was = box.is_selected()
        box.click()
        text_shown("Saved")
        cfg = Path(os.environ["XDG_CONFIG_HOME"], "tether/config.json")
        wait.until(lambda _: f'"auto_update": {str(not was).lower()}' in cfg.read_text())
        wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, "[data-key=set-auto_update]"))).click()
        wait.until(lambda _: f'"auto_update": {str(was).lower()}' in cfg.read_text())
    check("Settings page saves a switch", settings_page)

    def commands_editor():
        import json
        button("Settings").click()
        form = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=command-new]")))
        form.find_element(By.CSS_SELECTOR, "[aria-label='Command name']").send_keys("Test command")
        form.find_element(By.CSS_SELECTOR, "[aria-label='Shell command']").send_keys("true")
        button("Add command").click()
        cfg = Path(os.environ["XDG_CONFIG_HOME"], "tether/config.json")
        wait.until(lambda _: any(c["name"] == "Test command" for c in json.loads(cfg.read_text()).get("commands", [])))
        saved = wait.until(EC.visibility_of_element_located((By.XPATH, "//form[.//b[text()='Test command']]")))
        shell = saved.find_element(By.CSS_SELECTOR, "[aria-label='Shell command']")
        shell.clear()
        shell.send_keys("printf test")
        saved.find_element(By.XPATH, ".//button[text()='Save command']").click()
        wait.until(lambda _: any(c["command"] == "printf test" for c in json.loads(cfg.read_text()).get("commands", [])))
        saved = wait.until(EC.visibility_of_element_located((By.XPATH, "//form[.//b[text()='Test command']]")))
        saved.find_element(By.XPATH, ".//button[text()='Remove command']").click()
        wait.until(lambda _: not any(c["name"] == "Test command" for c in json.loads(cfg.read_text()).get("commands", [])))
    check("Commands editor adds, edits and removes", commands_editor)

    def rename_computer():
        # A new name restarts the background service; the window must come back by itself.
        name = wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, "[data-key=set-device_name]")))
        old = name.get_attribute("value")
        new = "Desk" if old != "Desk" else "Workstation"
        name.clear()
        name.send_keys(new, Keys.ENTER)
        long = WebDriverWait(d, 40)
        long.until(lambda _: d.execute_script("return state.me && state.me.deviceName") == new)
        long.until(lambda _: d.execute_script("return !!(device() && device().connected)"))
        long.until(lambda _: d.find_element(By.CSS_SELECTOR, "[data-key=set-device_name]").get_attribute("value") == new)
    check("renaming the computer restarts the service and reconnects", rename_computer)
finally:
    d.quit()

print(f"FAILED: {', '.join(failures)}" if failures else "PASSED: all checks")
sys.exit(1 if failures else 0)
