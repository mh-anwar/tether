"""Focused commands editor check, runnable with wd.sh with a remembered paired phone."""
import json, os
from pathlib import Path
from selenium import webdriver
from selenium.common.exceptions import StaleElementReferenceException, ElementClickInterceptedException
from selenium.webdriver.common.by import By
from selenium.webdriver.common.options import ArgOptions
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait
opts=ArgOptions()
opts.set_capability('browserName','wry')
opts.set_capability('tauri:options',{'application':os.environ['TETHER_APP']})
d=webdriver.Remote(command_executor=os.environ['WD_URL'],options=opts)
w=WebDriverWait(d,25,ignored_exceptions=(StaleElementReferenceException,ElementClickInterceptedException))
try:
 w.until(EC.element_to_be_clickable((By.XPATH,"//button[.//span[text()='Settings']]"))).click()
 form=w.until(EC.visibility_of_element_located((By.CSS_SELECTOR,'[data-key=command-new]')))
 form.find_element(By.CSS_SELECTOR,"[aria-label='Command name']").send_keys('Editor test')
 form.find_element(By.CSS_SELECTOR,"[aria-label='Shell command']").send_keys('true')
 form.find_element(By.XPATH,".//button[text()='Add command']").click()
 cfg=Path(os.environ['XDG_CONFIG_HOME'])/'tether/config.json'
 w.until(lambda _:any(c['name']=='Editor test' for c in json.loads(cfg.read_text())['commands']))
 form=w.until(EC.visibility_of_element_located((By.XPATH,"//form[.//b[text()='Editor test']]")))
 field=form.find_element(By.CSS_SELECTOR,"[aria-label='Shell command']");field.clear();field.send_keys('printf edited')
 form.find_element(By.XPATH,".//button[text()='Save command']").click()
 w.until(lambda _:any(c['command']=='printf edited' for c in json.loads(cfg.read_text())['commands']))
 form=w.until(EC.visibility_of_element_located((By.XPATH,"//form[.//b[text()='Editor test']]")))
 d.execute_script("document.getElementById('run-commands').scrollIntoView()")
 d.save_screenshot(str(Path(os.environ['XDG_CONFIG_HOME']).parent/'commands.png'))
 form.find_element(By.XPATH,".//button[text()='Remove command']").click()
 w.until(lambda _:not any(c['name']=='Editor test' for c in json.loads(cfg.read_text())['commands']))
 for key in ['allow_commands','allow_input','allow_lock','allow_volume','allow_ring']:
  w.until(lambda _:d.execute_script('return !settings.saving'))
  before=json.loads(cfg.read_text()).get(key,True)
  def toggle(_):
   field=d.find_element(By.CSS_SELECTOR,f'[data-key=set-{key}]')
   if field.is_selected()!=before:return True
   d.execute_script("arguments[0].scrollIntoView({block:'center'})",field)
   field.click()
   return True
  w.until(toggle)
  w.until(lambda _:d.execute_script(f'return !settings.saving && settings.data.{key} === {str(not before).lower()}'))
 print('PASSED: command add/edit/remove, persisted configuration, five permission switches')
finally:d.quit()
