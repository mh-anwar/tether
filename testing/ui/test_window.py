"""Window smoke test over WebDriver. Run with testing/ui/wd.sh (it starts everything this needs).

Finds things by role and text, the way a person would, so layout changes don't break it.
"""

import os
import sys
from pathlib import Path

from selenium import webdriver
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait
from selenium.common.exceptions import ElementClickInterceptedException, StaleElementReferenceException

opts = ArgOptions()
opts.set_capability("browserName", "wry")
opts.set_capability("tauri:options", {"application": os.environ["TETHER_APP"]})
d = webdriver.Remote(command_executor=os.environ["WD_URL"], options=opts)
# The window redraws regions on every event, so a found element can go stale between steps.
wait = WebDriverWait(d, 20, ignored_exceptions=(StaleElementReferenceException,))
failures = []


def check(name, fn):
    try:
        fn()
        print(f"ok    {name}")
    except Exception as e:  # report every step, not just the first failure
        import traceback
        traceback.print_exc(limit=3)
        failures.append(name)
        print(f"FAIL  {name}: {type(e).__name__} {str(e).splitlines()[0] if str(e) else ''}")


def button(text):
    return wait.until(EC.element_to_be_clickable(
        (By.XPATH, f"//button[normalize-space(.)='{text}' or .//span[normalize-space(.)='{text}']]")))


def click(text):
    # Find and click in one retried step: the button can be redrawn between the two.
    def go(_):
        button(text).click()
        return True
    wait.until(go)


def text_shown(text):
    # The element whose own text holds it, so hidden text elsewhere on the page doesn't count.
    wait.until(EC.visibility_of_element_located(
        (By.XPATH, f"//*[text()[contains(normalize-space(.), \"{text}\")]]")))


try:
    # Fresh isolated runs start with the incoming pairing request.
    import subprocess
    if not Path(os.environ["XDG_DATA_HOME"], "../phone/paired").resolve().exists():
        try:
            click("Accept")
        except Exception:
            pass
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
        click("Photos")
        wait.until(lambda _: len(d.find_elements(By.CSS_SELECTOR, ".photo-tile img")) == 5)
        d.find_element(By.XPATH, "//button[@role='tab'][contains(., 'Screenshots')]").click()
        wait.until(lambda _: len(d.find_elements(By.CSS_SELECTOR, ".photo-tile img")) == 2)
        d.find_element(By.CSS_SELECTOR, ".photo-tile").click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".viewer-body img")))
        click("Save to Downloads")
        text_shown("Saved to")
        d.find_element(By.CSS_SELECTOR, ".viewer button[aria-label='Close']").click()
    check("Photos page shows the phone's pictures and saves one", photos_page)

    def files_page():
        click("Files")
        wait.until(lambda _: not d.execute_script("return files.loading"))
        wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, ".storage-roots button"))).click()
        wait.until(EC.element_to_be_clickable((By.XPATH, "//button[@class='file-name' and text()='DCIM']"))).click()
        wait.until(EC.element_to_be_clickable((By.XPATH, "//button[@class='file-name' and text()='Camera']"))).click()
        wait.until(lambda _: len(d.find_elements(By.CSS_SELECTOR, ".file-row")) == 6)
        click("New folder")
        d.switch_to.active_element.send_keys("Window test", Keys.ENTER)
        wait.until(EC.element_to_be_clickable((By.XPATH, "//button[@class='file-name' and text()='Window test']")))
        row = d.find_element(By.XPATH, "//article[contains(@class,'file-row')][.//button[text()='Window test']]")
        row.find_element(By.XPATH, ".//button[text()='Rename']").click()
        field = d.switch_to.active_element
        field.send_keys(Keys.CONTROL, "a")
        field.send_keys("Renamed folder", Keys.ENTER)
        wait.until(EC.element_to_be_clickable((By.XPATH, "//button[@class='file-name' and text()='Renamed folder']")))
        row = d.find_element(By.XPATH, "//article[contains(@class,'file-row')][.//button[text()='Renamed folder']]")
        row.find_element(By.XPATH, ".//button[text()='Delete']").click()
        d.find_element(By.CSS_SELECTOR, "dialog .danger").click()
        wait.until(lambda _: not d.find_elements(By.XPATH, "//button[text()='Renamed folder']"))
        upload = Path(os.environ["XDG_DATA_HOME"], "upload test.txt")
        upload.write_text("Uploaded from the computer")
        d.execute_script("uploadFile(arguments[0])", str(upload))
        item = wait.until(EC.element_to_be_clickable((By.XPATH, "//button[@class='file-name' and text()='upload test.txt']")))
        row = item.find_element(By.XPATH, "ancestor::article")
        row.find_element(By.XPATH, ".//button[text()='Save']").click()
        wait.until(lambda _: Path(os.environ["XDG_DATA_HOME"], "../dl/upload test.txt").resolve().exists())
        row.find_element(By.XPATH, ".//button[text()='Delete']").click()
        d.find_element(By.CSS_SELECTOR, "dialog .danger").click()
        wait.until(lambda _: not d.find_elements(By.XPATH, "//button[text()='upload test.txt']"))
    check("Files browse, create, rename, upload, save, delete", files_page)

    def contacts_page():
        click("Contacts")
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".contact-card")))
        name = d.find_element(By.CSS_SELECTOR, ".contact-card h2").text
        d.find_element(By.CSS_SELECTOR, "[aria-label='Search contacts']").send_keys(name)
        click("Text")
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, ".composer textarea")))
        assert d.execute_script("return state.newTo.length") == 1
    check("Contacts search and Text starts a conversation", contacts_page)

    def keyboard_page():
        click("Keyboard")
        text_shown("Pick KDE Connect Remote Keyboard")
        box = d.find_element(By.ID, "keyboard-box")
        box.send_keys("Hello", Keys.ARROW_LEFT, Keys.BACKSPACE, Keys.ENTER)
        text_shown("Received by your phone")
        log = Path(os.environ["FAKE_MOUSEPAD_LOG"])
        import json
        wait.until(lambda _: log.exists() and len(log.read_text().splitlines()) >= 8)
        packets = [json.loads(line) for line in log.read_text().splitlines()]
        assert any(p.get("key") == "H" for p in packets)
        assert any(p.get("specialKey") == 4 for p in packets)
        assert any(p.get("specialKey") == 12 for p in packets)
    check("Keyboard streams text and special keys with acknowledgements", keyboard_page)

    def device_card():
        click("Devices")
        text_shown("Battery 82%")
        text_shown("5G signal")
        assert d.find_elements(By.XPATH, "//button[text()='Forget phone']")
    check("Devices page shows battery, signal and forget", device_card)

    def readiness_checklist():
        click("Devices")
        text_shown("On your phone")
        text_shown("needs its switch on")
        text_shown("Battery > Unrestricted")
        rows = d.find_elements(By.CSS_SELECTOR, ".readiness-list li")
        assert len(rows) == 17
        texts = d.find_element(By.CSS_SELECTOR, '[data-feature="texts"]')
        if "kdeconnect.sms.messages" in os.environ.get("FAKE_PHONE_OMIT_CAPABILITIES", ""):
            assert "Not on this phone" in texts.text
        else:
            # The stand-in phone has sent conversations, so texts are proven to work.
            assert "Working" in texts.text
            # The stand-in phone also reported its battery, so that must not wait.
            assert "Working" in d.find_element(By.CSS_SELECTOR, '[data-feature="battery"]').text
        assert texts.find_elements(By.CSS_SELECTOR, "svg")
        # With nothing received yet, each page points at the right switch on the phone.
        d.execute_script("window.savedContacts = state.contacts; window.savedThreads = state.threads; window.savedKeyboard = state.keyboardActive; state.threads = new Map(); state.keyboardActive = false; renderList()")
        try:
            for view, feature in [("messages", "Send SMS"), ("contacts", "Contacts Synchronizer"), ("keyboard", "Keyboard receiver")]:
                d.execute_script("showView(arguments[0])", view)
                if view == "contacts":
                    d.execute_script("state.contacts = []; contactsQuery = ''; renderTools()")
                text_shown(feature)
            d.execute_script("window.clickSftp = device().readiness.find(f => f.id === 'sftp'); clickSftp.ready = false; showView('files')")
            text_shown("does not have this")
        finally:
            d.execute_script("clickSftp.ready = true; state.keyboardActive = savedKeyboard; state.threads = savedThreads; state.contacts = savedContacts; showView('devices')")
    check("phone capability checklist and shared setup hints", readiness_checklist)

    def screenshots():
        # The window's own pixels (no desktop around it); $TETHER_SHOTS picks the folder.
        output = Path(os.environ.get("TETHER_SHOTS") or Path(os.environ["XDG_CONFIG_HOME"]).parent / "shots")
        output.mkdir(parents=True, exist_ok=True)
        d.execute_script("contactsQuery = ''; state.selected = 1; document.querySelectorAll('.toast').forEach((t) => t.remove())")
        d.set_window_size(1280, 800)
        views = [("messages", "messages"), ("files", "files-page"), ("contacts", "contacts-page"), ("keyboard", "keyboard"),
                 ("devices", "device-card"), ("photos", "photos-page"), ("settings", "settings-page")]
        for theme in ["light", "dark"]:
            d.execute_script("document.documentElement.dataset.theme = arguments[0]", theme)
            for view, name in views:
                d.execute_script("showView(arguments[0])", view)
                wait.until(lambda _: d.execute_script("return state.view !== 'files' || !files.loading"))
                # Park the pointer on the page title so no nav item shows its hover colour.
                titles = d.find_elements(By.CSS_SELECTOR, "h1")
                visible = [t for t in titles if t.is_displayed()]
                if visible:
                    ActionChains(d).move_to_element(visible[0]).perform()
                if view == "photos":
                    __import__("time").sleep(3)  # previews arrive a few at a time
                d.save_screenshot(str(output / f"{name}{'-dark' if theme == 'dark' else ''}.png"))
                if view == "devices":
                    d.save_screenshot(str(output / ("readiness-dark.png" if theme == "dark" else "readiness.png")))
        d.execute_script("delete document.documentElement.dataset.theme")
        for width in [720, 1000]:
            d.set_window_size(width, 780)
            for view, name in [("messages", "narrow"), ("files", "narrow-files"), ("devices", "narrow-devices")]:
                d.execute_script("showView(arguments[0])", view)
                wait.until(lambda _: d.execute_script("return state.view !== 'files' || !files.loading"))
                assert d.execute_script("return document.documentElement.scrollWidth <= innerWidth"), (view, width)
                d.save_screenshot(str(output / f"{name}-{width}.png"))
        d.set_window_size(1200, 780)
    check("new pages and narrow layouts screenshots", screenshots)

    def apps_page():
        click("Apps")
        text_shown("phone's apps")
    check("Apps page opens", apps_page)

    def code_form():
        Path(os.environ["FAKE_APPS_DIR"], "codeopen").touch()
        click("Pair with a code instead")
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=apps-code]")))
        text_shown("Wi-Fi pairing code")
    check("code pairing form shows", code_form)

    def pair_for_apps():
        wait.until(lambda _: d.execute_script("return apps.status?.pairingAddr"))
        d.find_element(By.CSS_SELECTOR, "[data-key=apps-code]").send_keys("123456")
        click("Pair")
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

    def app_window_open_and_close():
        fake = Path(os.environ["FAKE_APPS_DIR"])
        calc = "com.sec.android.app.popupcalculator"
        wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, f"[data-key='app-{calc}']"))).click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, f"[data-key='close-{calc}']")))
        text_shown("Open now")
        # A second click doesn't start a second window.
        wait.until(EC.element_to_be_clickable((By.CSS_SELECTOR, f"[data-key='app-{calc}']"))).click()
        text_shown("already open")
        assert (fake / "calls.log").read_text().count(f"--start-app={calc}") == 1
        click_key = lambda _: d.find_element(By.CSS_SELECTOR, f"[data-key='close-{calc}']").click() or True
        wait.until(click_key)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "[data-key=apps-open-windows]"))
    check("an app opens once in its own window and closes from the Apps page", app_window_open_and_close)

    def old_android_note():
        fake = Path(os.environ["FAKE_APPS_DIR"])
        (fake / "sdk").write_text("28")
        (fake / "connected").unlink()
        d.execute_script("pollApps()")
        wait.until(lambda _: d.execute_script("return !apps.status?.device"))
        wait.until(lambda _: (fake / "connected").exists())
        d.execute_script("pollApps()")
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=apps-shared-note]")))
        text_shown("Android 9")
        (fake / "sdk").unlink()
    check("Android 9 phones get a plain note, not app windows", old_android_note)

    def palette():
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "k")
        wait.until(EC.visibility_of_element_located((By.XPATH, "//*[contains(@class, 'palette')]//*[normalize-space(.)='Check for updates']")))
        d.switch_to.active_element.send_keys(Keys.ESCAPE)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, ".palette-backdrop"))
    check("Ctrl+K offers Check for updates", palette)

    def settings_page():
        click("Settings")
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
        click("Settings")
        form = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=command-new]")))
        form.find_element(By.CSS_SELECTOR, "[aria-label='Command name']").send_keys("Test command")
        form.find_element(By.CSS_SELECTOR, "[aria-label='Shell command']").send_keys("true")
        click("Add command")
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

    def tray_note():
        # wd.sh runs the service as on GNOME with no tray; Settings explains once, until closed.
        import json
        click("Settings")
        d.execute_script("return api('settings_set', { tray_note_dismissed: false }).then(() => loadSettings())")
        note = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=tray-note]")))
        assert "extensions.gnome.org/extension/615" in note.text, note.text
        assert note.find_element(By.CSS_SELECTOR, "[data-key=tray-extension]").is_enabled()
        output = Path(os.environ.get("TETHER_SHOTS") or Path(os.environ["XDG_CONFIG_HOME"]).parent / "shots")
        output.mkdir(parents=True, exist_ok=True)
        d.save_screenshot(str(output / "settings-tray-note.png"))

        # Toasts from earlier steps can sit over the button for a moment; retry until they go.
        def close(_):
            d.find_element(By.CSS_SELECTOR, "[data-key=tray-note-close]").click()
            return True
        WebDriverWait(d, 20, ignored_exceptions=(StaleElementReferenceException, ElementClickInterceptedException)).until(close)
        cfg = Path(os.environ["XDG_CONFIG_HOME"], "tether/config.json")
        wait.until(lambda _: json.loads(cfg.read_text()).get("tray_note_dismissed") is True)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "[data-key=tray-note]"))
    check("Settings explains the missing tray on GNOME, once", tray_note)

    def rename_computer():
        # A new name restarts the background service; the window must come back by itself.
        field = (By.CSS_SELECTOR, "[data-key=set-device_name]")
        old = wait.until(EC.element_to_be_clickable(field)).get_attribute("value")
        new = "Desk" if old != "Desk" else "Workstation"

        def type_name(_):
            name = d.find_element(*field)
            name.clear()
            name.send_keys(new, Keys.ENTER)
            return True
        wait.until(type_name)
        long = WebDriverWait(d, 40, ignored_exceptions=(StaleElementReferenceException,))
        long.until(lambda _: d.execute_script("return state.me && state.me.deviceName") == new)
        long.until(lambda _: d.execute_script("return !!(device() && device().connected)"))
        long.until(lambda _: d.find_element(By.CSS_SELECTOR, "[data-key=set-device_name]").get_attribute("value") == new)
    check("renaming the computer restarts the service and reconnects", rename_computer)
finally:
    d.quit()

print(f"FAILED: {', '.join(failures)}" if failures else "PASSED: all checks")
sys.exit(1 if failures else 0)
