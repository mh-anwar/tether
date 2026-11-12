"""Window smoke test over WebDriver. Run with testing/ui/wd.sh (it starts everything this needs).

Finds things by role and text, the way a person would, so layout changes don't break it.
"""

import os
import sys

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

    def apps_page():
        button("Apps").click()
        text_shown("phone's apps")
    check("Apps page opens", apps_page)

    def code_form():
        button("Pair with a code instead").click()
        wait.until(EC.visibility_of_element_located((By.CSS_SELECTOR, "[data-key=apps-code]")))
        text_shown("Wi-Fi pairing code")
    check("code pairing form shows", code_form)

    def palette():
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.CONTROL, "k")
        wait.until(EC.visibility_of_element_located((By.XPATH, "//*[normalize-space(.)='Check for updates']")))
        d.find_element(By.TAG_NAME, "body").send_keys(Keys.ESCAPE)
    check("Ctrl+K offers Check for updates", palette)
finally:
    d.quit()

print(f"FAILED: {', '.join(failures)}" if failures else "PASSED: all checks")
sys.exit(1 if failures else 0)
