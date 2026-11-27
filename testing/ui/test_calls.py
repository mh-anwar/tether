"""Calls through this computer, against the stand-in PipeWire telephony service (fake_telephony.py).

Run: testing/ui/wd.sh testing/ui/test_calls.py   (wd.sh gives the window its own session bus)
Covers: the setup steps before Bluetooth calling is ready, an incoming call answered and hung up from
the banner, moving the sound to the computer, and dialing from the command palette.
"""

import os
import subprocess
import sys
from pathlib import Path

from selenium import webdriver
from selenium.common.exceptions import StaleElementReferenceException
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait

HERE = Path(__file__).parent
SHOTS = Path(os.environ.get("TETHER_SHOTS", Path(os.environ["XDG_CONFIG_HOME"]).parent))
phone = subprocess.Popen(
    ["uv", "run", "-q", "--no-project", "--with", "dbus-fast", "python", str(HERE / "fake_telephony.py")],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
assert phone.stdout.readline().strip() == "ready", "fake telephony didn't start"


def tell(cmd):
    phone.stdin.write(cmd + "\n")
    phone.stdin.flush()


def heard(prefix):
    # Reads the fake's log until the line we expect: it prints every method the window called.
    for line in phone.stdout:
        if line.startswith(prefix):
            return line.strip()
    raise AssertionError(f"phone never got {prefix}")


opts = ArgOptions()
opts.set_capability("browserName", "wry")
opts.set_capability("tauri:options", {"application": os.environ["TETHER_APP"]})
d = webdriver.Remote(command_executor=os.environ["WD_URL"], options=opts)
wait = WebDriverWait(d, 20, ignored_exceptions=(StaleElementReferenceException,))
failures = []


def check(name, fn):
    try:
        fn()
        print(f"ok    {name}")
    except Exception as e:
        import traceback
        traceback.print_exc(limit=3)
        failures.append(name)
        print(f"FAIL  {name}: {type(e).__name__} {str(e).splitlines()[0] if str(e) else ''}")


def banner_button(text):
    return wait.until(EC.element_to_be_clickable(
        (By.XPATH, f"//*[@id='call-root']//button[normalize-space(.)='{text}']")))


def mic_state():
    try:
        return Path(os.environ["FAKE_WPCTL_STATE"]).read_text().strip()
    except FileNotFoundError:
        return ""


def text_shown(text):
    return wait.until(EC.visibility_of_element_located(
        (By.XPATH, f"//*[text()[contains(normalize-space(.), \"{text}\")]]")))


try:
    if not Path(os.environ["XDG_DATA_HOME"], "../phone/paired").resolve().exists():
        try:
            wait.until(EC.element_to_be_clickable((By.XPATH, "//button[normalize-space(.)='Accept']"))).click()
        except Exception:
            pass
    wait.until(lambda _: d.execute_script("return state.contacts.length > 0"))

    def setup_steps():
        # No gateway yet and no BlueZ on this bus: the Call button explains what's missing.
        wait.until(lambda _: d.execute_script("return calls.status && calls.status.blocker && calls.status.blocker !== 'service'"))
        c = d.execute_script("return state.contacts[0]")
        d.execute_script("dialNumber(arguments[0].number, arguments[0].name)", c)
        text_shown("Calls from this computer")
        d.save_screenshot(str(SHOTS / "call-setup.png"))
        assert d.find_elements(By.CSS_SELECTOR, ".call-dialog ol.steps li")
        d.switch_to.active_element.send_keys(Keys.ESCAPE)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, ".call-dialog"))
    check("setup steps show while Bluetooth calling isn't ready", setup_steps)

    def settings_not_ready():
        # Settings, Calls says calls aren't set up and leads to the same steps.
        d.execute_script("showView('settings')")
        wait.until(EC.element_to_be_clickable((By.XPATH, "//*[@role='tab' and normalize-space(.)='Calls']"))).click()
        text_shown("Not set up yet")
        wait.until(EC.element_to_be_clickable((By.XPATH, "//button[normalize-space(.)='Show the steps']"))).click()
        text_shown("Calls from this computer")
        d.switch_to.active_element.send_keys(Keys.ESCAPE)
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, ".call-dialog"))
    check("Settings, Calls explains what's missing", settings_not_ready)

    def incoming():
        tell("gateway")
        wait.until(lambda _: d.execute_script("return calls.status && calls.status.blocker === null"))
        # Settings, Calls (still open) turns to Ready on its own.
        text_shown("Ready")
        assert d.find_elements(By.XPATH, "//button[normalize-space(.)='Call a number']")
        d.save_screenshot(str(SHOTS / "call-settings-ready.png"))
        d.execute_script("showView('messages')")
        c = d.execute_script("return state.contacts[0]")
        tell(f"ring {c['number']}")
        banner_button("Answer")
        # The caller's name comes from the phone's contacts.
        text_shown(c["name"])
        text_shown("Incoming call")
        assert not d.find_elements(By.CSS_SELECTOR, ".rail .call:not(.finding)"), "rail banner duplicated"
        d.save_screenshot(str(SHOTS / "call-incoming.png"))
        banner_button("Answer").click()
        heard("called Answer")
        text_shown("Sound on your phone")
        banner_button("Use this computer").click()
        heard("called Activate")
        text_shown("Sound on this computer")
        wait.until(lambda _: d.find_element(By.CSS_SELECTOR, ".call-timer").text not in ("", "00:00"))
        d.save_screenshot(str(SHOTS / "call-active.png"))
        # Mute turns off this computer's microphone and says so in words, not only by colour.
        banner_button("Mute").click()
        wait.until(lambda _: mic_state() == "1")
        unmute = banner_button("Unmute")
        assert unmute.get_attribute("aria-pressed") == "true"
        text_shown("Microphone off")
        d.save_screenshot(str(SHOTS / "call-muted.png"))
        # And from the keyboard, through Ctrl+K.
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "k")
        field = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[aria-label='Command']")))
        field.send_keys("Unmute the microphone", Keys.ENTER)
        wait.until(lambda _: mic_state() == "0")
        banner_button("Mute").click()
        wait.until(lambda _: mic_state() == "1")
        # The keypad sends keys into the call for phone menus, by click and by typing.
        banner_button("Keypad").click()
        d.find_element(By.XPATH, "//*[@id='call-root']//*[@role='group' and @aria-label='Keypad']//button[@aria-label='1']").click()
        assert heard("called SendTones") == "called SendTones 1"
        d.switch_to.active_element.send_keys("#")
        assert heard("called SendTones") == "called SendTones #"
        text_shown("1#")
        d.save_screenshot(str(SHOTS / "call-keypad.png"))
        d.switch_to.active_element.send_keys(Keys.ESCAPE)
        banner_button("Keypad")
        banner_button("Hang up").click()
        heard("called Hangup")
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "#call-root .call-card"))
        # Tether muted it for the call, so the call ending turns the microphone back on.
        wait.until(lambda _: mic_state() == "0")
    check("incoming call: answer, sound here, timer, mute, keypad, hang up", incoming)

    def decline():
        tell("ring +15550199")
        banner_button("Decline").click()
        heard("called Hangup")
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "#call-root .call-card"))
    check("incoming call: decline", decline)

    def dial_from_palette():
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "k")
        field = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[aria-label='Command']")))
        field.send_keys("Call a number", Keys.ENTER)
        number = wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[aria-label='Phone number']")))
        number.send_keys("(555) 010-9999", Keys.ENTER)
        assert heard("called Dial") == "called Dial 5550109999"
        text_shown("Calling")
        text_shown("Sound on")  # the callee picks up after a moment
        banner_button("Hang up").click()
        heard("called Hangup")
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "#call-root .call-card"))
    check("dial a number from the command palette", dial_from_palette)

    def far_end_hangs_up():
        tell("ring +15550123")
        banner_button("Answer").click()
        heard("called Answer")
        tell("remote_hangup")
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "#call-root .call-card"))
    check("the other side hanging up clears the banner", far_end_hangs_up)

    def call_waiting():
        tell("ring +15550123")
        banner_button("Answer").click()
        heard("called Answer")
        text_shown("Sound on")
        tell("ring +15550456")  # rings as "waiting" while the first call is active
        text_shown("Call waiting")
        banner_button("Answer").click()
        heard("called HoldAndAnswer")
        text_shown("On hold")
        d.save_screenshot(str(SHOTS / "call-two.png"))
        # Only the call being talked on offers Mute.
        assert len(d.find_elements(By.XPATH, "//*[@id='call-root']//button[normalize-space(.)='Mute']")) == 1
        banner_button("Switch to this call").click()
        heard("called HoldAndAnswer")
        wait.until(lambda _: d.find_elements(By.XPATH, "//*[@id='call-root']//button[normalize-space(.)='Switch to this call']"))
        tell("remote_hangup")
        wait.until(lambda _: not d.find_elements(By.CSS_SELECTOR, "#call-root .call-card"))
    check("call waiting: answer holds the first, switch back", call_waiting)

    def missed_call_text():
        # "Send a text" on a missed-call pop-up opens a new message to that number.
        d.execute_script("onEvent({event: 'compose', number: '+15550789', name: ''})")
        wait.until(lambda _: d.execute_script("return state.selected === 'new' && state.newTo[0] === '+15550789'"))
    check("missed call: Send a text opens a new message", missed_call_text)

    def dark_banner():
        tell("ring +15550123")
        banner_button("Answer")
        d.execute_script("document.documentElement.dataset.theme = 'dark'")
        d.save_screenshot(str(SHOTS / "call-incoming-dark.png"))
        d.execute_script("delete document.documentElement.dataset.theme")
        tell("remote_hangup")
    check("dark screenshot", dark_banner)
finally:
    d.quit()
    phone.kill()

if failures:
    print(f"{len(failures)} failed: {', '.join(failures)}")
    sys.exit(1)
print("PASSED: calls setup, settings, answer, sound here, mute, keypad, hang up, decline, dial, far-end hang up, call waiting, missed-call text")
