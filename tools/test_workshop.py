"""Headless Skin Workshop checks. Run: python tools\\test_workshop.py"""

import mimetypes
import base64
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright


ROOT = Path(__file__).resolve().parents[1]
ORIGIN = "http://workshop.test/"


def serve(route):
    if route.request.url.startswith("https://avatars.steamstatic.com/"):
        route.fulfill(body=base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGNgAAIAAAUAAXpeqz8AAAAASUVORK5CYII="), content_type="image/png")
        return
    name = urlsplit(route.request.url).path.lstrip("/")
    if name not in ("workshop.html", "workshop.js", "workshop.css"):
        raise AssertionError(f"Unexpected browser request: {route.request.url}")
    path = ROOT / "ui" / name
    content_type = "text/javascript" if path.suffix == ".js" else mimetypes.guess_type(path)[0]
    route.fulfill(body=path.read_bytes(), content_type=content_type)


MOCK = """
window.workshopMock = {
  calls: [], connected: false, moderator: false, failPublish: false, failModeration:false,
  submissions:[{id:'d'.repeat(32),title:'Queued test skin',description:'Waiting for checks',model:'models/test.mdl',
    steamId:'76561198000000000',uploaderName:'Example uploader',uploaderAvatar:'https://avatars.steamstatic.com/'+'a'.repeat(40)+'_medium.jpg',status:'validating',created:1700000000}],
  bans:[{steamId:'76561198000000002',name:'Banned uploader',reason:'Abusive uploads',created:1700000000}],
  pending: {id:'local-draft', revision:'revision-1', title:'Selected draft',
    preview:'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==',
    draft:{textures:[{}], modelPath:'models/test.mdl'}},
  skin: {id:'a'.repeat(32), title:'<img src=x onerror=alert(1)>', description:'<script>window.injected=true</script>',
    steamId:'76561198000000000', uploaderName:'Example uploader',uploaderAvatar:'https://avatars.steamstatic.com/'+'a'.repeat(40)+'_medium.jpg',model:'models/test.mdl', license:'CC-BY-4.0', hash:'b'.repeat(64), previewHash:'c'.repeat(64)}
};
window.__TAURI__ = {core:{invoke:async (command, args={}) => {
  const m = window.workshopMock;
  m.calls.push({command, args});
  switch (command) {
    case 'workshop_list': return {skins:[m.skin], next:null};
    case 'workshop_submissions': return {submissions:m.submissions};
    case 'workshop_bans': return {bans:m.bans,next:null};
    case 'workshop_pending': return m.pending;
    case 'workshop_discard': m.pending = null; return;
    case 'workshop_preview': return m.pending?.preview || '';
    case 'workshop_download': return 'Downloaded community draft';
    case 'workshop_signin': m.connected = true; return;
    case 'workshop_account': return m.connected
      ? {steamId:'76561198000000000',name:'Example uploader',avatar:'https://avatars.steamstatic.com/'+'a'.repeat(40)+'_medium.jpg',
        moderator:m.moderator} : {connected:false};
    case 'workshop_signout': m.connected = false; return;
    case 'workshop_publish':
      if (m.failPublish) throw Error('Simulated service rejection');
      m.pending = null; return {id:'a'.repeat(32), status:'validating'};
    case 'workshop_moderate':
      if (m.failModeration) throw Error('Simulated moderation rejection');
      if (args.action === 'cancel') m.submissions = m.submissions.map(item => item.id === args.id ? {...item,status:'cancelled'} : item);
      if (args.action === 'remove') m.submissions = m.submissions.filter(item => item.id !== args.id);
      if (args.action === 'unban') m.bans = m.bans.filter(item => item.steamId !== args.steamId);
      if (args.action === 'ban') m.bans.push({steamId:args.steamId,reason:args.reason,created:1700000000});
      return;
    default: throw Error('Unexpected native command: ' + command);
  }
}}};
"""

with sync_playwright() as playwright:
    browser = playwright.chromium.launch(channel="msedge", headless=True)
    try:
        page = browser.new_page(viewport={"width": 1100, "height": 780})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.route("**/*", serve)
        page.add_init_script(MOCK)
        page.clock.install()
        def confirm_click(locator):
            locator.click()
            page.locator("#confirmation").wait_for(state="visible")
            page.locator("#confirmation button[value=confirm]").click()
        page.goto(ORIGIN + "workshop.html")
        page.wait_for_selector(".card")
        page.wait_for_function("() => workshopMock.calls.some(c => c.command === 'workshop_preview')")
        assert page.locator(".card h2").inner_text() == "<img src=x onerror=alert(1)>"
        assert not page.evaluate("window.injected || false")
        assert not page.locator("#tab-bans").is_visible()
        assert not page.locator("#tab-submissions").is_visible()
        assert page.evaluate("workshopMock.calls.filter(c => c.command === 'workshop_publish').length") == 0
        assert not page.locator("#publish").is_visible()
        assert not page.evaluate("workshopMock.calls.some(c => c.command === 'workshop_pending')")
        page.locator("#signin").click()
        page.clock.fast_forward(3000)
        page.wait_for_selector("#publish")

        page.locator("#title").fill("My edited title")
        page.locator("#description").fill("Keep this description")
        page.locator("#consent").check()
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        page.wait_for_timeout(50)
        assert page.locator("#title").input_value() == "My edited title"
        assert page.locator("#description").input_value() == "Keep this description"
        assert page.locator("#consent").is_checked()
        staged = page.evaluate("workshopMock.pending")
        page.locator("#cancel-publish").click()
        page.wait_for_function("() => document.querySelector('#publish').hidden")
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        assert not page.locator("#publish").is_visible()
        assert not page.evaluate("workshopMock.calls.some(c => /delete|install|publish$/.test(c.command))")
        page.evaluate("(value) => {workshopMock.pending=value; window.dispatchEvent(new Event('focus'))}", staged)
        page.wait_for_selector("#publish")
        page.locator("#consent").check()

        page.wait_for_function("() => document.querySelector('#signout').hidden === false")
        assert page.locator("#account").inner_text() == "Example uploader"
        assert page.locator("#account-avatar").get_attribute("src").startswith("https://avatars.steamstatic.com/")
        assert "Example uploader" in page.locator("#cards").inner_text()
        page.locator("#tab-submissions").click()
        page.wait_for_selector("#submission-list .card")
        assert page.locator("#submissions-title").inner_text() == "Your pending submissions"
        assert page.get_by_role("button", name="Cancel submission", exact=True).count() == 1
        assert page.locator("#submission-list .uploader-avatar").count() == 1
        assert page.locator("#cards .uploader-avatar").count() == 1
        page.get_by_role("button", name="Cancel submission", exact=True).click()
        assert page.locator("#confirmation button[value=cancel]").inner_text() == "Cancel"
        assert "danger" in page.locator("#confirmation button[value=cancel]").get_attribute("class")
        page.locator("#confirmation button[value=cancel]").click()
        assert not page.evaluate("workshopMock.calls.some(c => c.command === 'workshop_moderate')")
        assert not page.locator("#community-panel").is_visible()
        page.evaluate("workshopMock.submissions.push({...workshopMock.submissions[0],id:'e'.repeat(32),status:'failed',failureReason:'Texture dimensions do not match approved content'})")
        page.locator("#refresh").click()
        page.wait_for_function("() => document.querySelector('#submission-list').textContent.includes('Failure reason: Texture dimensions')")
        confirm_click(page.get_by_role("button", name="Remove upload", exact=True))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('Upload removed')")
        assert page.locator("#submission-list .card").count() == 1
        page.locator("#tab-community").click()
        page.locator("#consent").uncheck()
        page.locator("#submit").click()
        assert "Confirm sharing permission" in page.locator("#status").inner_text()
        page.get_by_role("button", name="Download draft", exact=True).click()
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('added to Custom skins')")
        assert not page.evaluate("workshopMock.calls.some(c => /install|restore/i.test(c.command))")

        previews_before = page.evaluate("workshopMock.calls.filter(c => c.command === 'workshop_preview').length")
        for value in ["test", "no matches", "test"]:
            page.locator("#search").fill(value)
        page.wait_for_timeout(100)
        assert page.evaluate("workshopMock.calls.filter(c => c.command === 'workshop_preview').length") == previews_before

        page.evaluate("workshopMock.failPublish = true")
        page.locator("#consent").check()
        confirm_click(page.locator("#submit"))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('Simulated service rejection')")
        assert page.locator("#publish").is_visible()
        assert not page.locator("#consent").is_checked()
        assert page.evaluate("workshopMock.calls.filter(c => c.command === 'workshop_publish').length") == 1
        assert page.evaluate("workshopMock.calls.find(c => c.command === 'workshop_publish').args.submission.revision") == "revision-1"

        page.evaluate("workshopMock.pending = {...workshopMock.pending, revision:'revision-2', title:'Changed draft'}")
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        page.wait_for_function("() => document.querySelector('#title').value === 'Changed draft'")
        assert page.locator("#description").input_value() == ""
        page.evaluate("workshopMock.failPublish = false")
        page.locator("#consent").check()
        confirm_click(page.locator("#submit"))
        page.wait_for_function("() => document.querySelector('#publish').hidden")
        assert "not public yet" in page.locator("#status").inner_text()
        assert page.locator("#submissions").is_visible()
        page.locator("#signout").click()
        assert not page.locator("#tab-submissions").is_visible()
        assert page.locator("#community-panel").is_visible()
        page.evaluate("workshopMock.moderator = true")
        page.locator("#signin").click()
        page.clock.fast_forward(3000)
        page.wait_for_function("() => !document.querySelector('#tab-bans').hidden")
        page.locator("#tab-submissions").click()
        page.wait_for_selector("#submission-list .card")
        assert page.locator("#submissions-title").inner_text() == "All pending submissions"
        page.evaluate("workshopMock.failModeration = true")
        confirm_click(page.get_by_role("button", name="Cancel submission", exact=True))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('Cancellation failed')")
        assert page.get_by_role("button", name="Cancel submission", exact=True).is_enabled()
        page.evaluate("workshopMock.failModeration = false")
        confirm_click(page.get_by_role("button", name="Cancel submission", exact=True))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('Submission cancelled')")
        assert "Cancelled" in page.locator("#submission-list").inner_text()
        confirm_click(page.get_by_role("button", name="Remove upload", exact=True))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('Upload removed')")
        assert page.locator("#submission-list .card").count() == 0
        page.locator("#tab-bans").click()
        page.wait_for_selector("#ban-list tr")
        assert "Banned uploader" in page.locator("#ban-list").inner_text()
        confirm_click(page.get_by_role("button", name="Unban", exact=True))
        page.wait_for_function("() => document.querySelector('#status').textContent.includes('unbanned')")
        assert page.locator("#ban-empty").is_visible()
        page.locator("#ban-id").fill("76561198000000003")
        page.locator("#ban-reason").fill("Repeated abusive uploads")
        confirm_click(page.locator("#ban"))
        page.wait_for_selector("#ban-list tr")
        assert "Repeated abusive uploads" in page.locator("#ban-list").inner_text()
        page.locator("#search").fill("does not match")
        assert page.locator("#ban-list tr").count() == 0
        page.locator("#signout").click()
        assert page.locator("#ban-list tr").count() == 0
        assert not page.locator("#tab-bans").is_visible()
        assert not errors, errors
        print("PASS: safe metadata, Steam/consent gates, focus-preserved form, cached previews, draft-only downloads, explicit failure and reviewed snapshot")
        print("PASS: named Steam account, uploader names, scoped pending tab, moderator cancellation, ban table/search/unban and sign-out privacy")
    finally:
        browser.close()
