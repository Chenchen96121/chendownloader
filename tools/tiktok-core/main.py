"""Private stdin/JSONL executable entry point; no browser or account state access."""
import json
import re
import sys
import time
from urllib.parse import urlsplit

from assets import preview,stream_file,probe_file
from detail import normalize_detail
from protocol import VERSION,RequestError,emit,read_request
from session import TikTokSession,UA
from signer import sign
from url_policy import normalize_url,POST

def analyze(session,url):
    initial=POST.fullmatch(urlsplit(url).path)
    try: page,final_url,_=session.read(url)
    except RequestError as error:
        if not initial or error.code not in ('authentication_required','access_denied','content_unavailable','network_failed'): raise
        # Page restrictions are not proof that the public detail endpoint is unavailable.
        page,final_url=b'',url
    canonical=normalize_url(final_url)
    match=POST.fullmatch(urlsplit(canonical).path)
    if not match: raise RequestError('unsupported_media')
    post_id=match.group(3)
    text=page.decode('utf-8',errors='replace')
    wid=re.search(r'"(?:wid|webId)"\s*:\s*"(\d{15,22})"',text)
    token=next((c.value for c in session.client.cookies.jar if c.name=='msToken' and c.domain.lstrip('.') in ('tiktok.com','www.tiktok.com')),'')
    pairs={'WebIdLastTime':str(int(time.time())),'aid':'1988','app_language':'en','app_name':'tiktok_web',
           'browser_language':'en-US','browser_name':'Mozilla','browser_online':'true','browser_platform':'MacIntel',
           'browser_version':UA[8:],'channel':'tiktok_web','cookie_enabled':'true','device_id':wid.group(1) if wid else '',
           'device_platform':'web_pc','focus_state':'true','from_page':'video','history_len':'2','is_fullscreen':'false',
           'is_page_visible':'true','itemId':post_id,'language':'en','os':'mac','priority_region':'US','referer':'',
           'region':'US','screen_height':'864','screen_width':'1536','tz_name':'Asia/Shanghai','user_is_login':'false','webcast_language':'en'}
    query,_=sign(pairs.items(),UA,ms_token=token)
    body,_,_=session.read('https://www.tiktok.com/api/item/detail/?'+query,headers={'Referer':canonical,'Accept':'application/json'})
    try: raw=json.loads(body)
    except (ValueError,UnicodeError): raise RequestError('extractor_outdated') from None
    item=(raw.get('itemInfo') or {}).get('itemStruct')
    if not item:
        code=raw.get('statusCode',raw.get('status_code'))
        raise RequestError('authentication_required' if code in (10216,10222) else 'access_denied' if code==10204 else 'content_unavailable')
    result=normalize_detail(item,post_id)
    result['sourcePageUrl']=canonical
    result['context']=session.context()
    return result

def run(request,output):
    operation=request['operation']
    url=normalize_url(request['url'])
    session=TikTokSession(request['network'],request.get('context'),seconds=15 if operation in ('preview','probe') else 120 if operation=='analyze' else 24*3600)
    try:
        if operation=='analyze': result=analyze(session,url)
        elif operation=='preview': result=preview(session,request['asset'],url)
        elif operation=='probe': result=probe_file(session,request['asset'],request.get('stagingPath',''),url)
        else:
            result=stream_file(session,request['asset'],request.get('stagingPath',''),url,
                               lambda downloaded,total:emit(output,'progress',{'downloadedBytes':downloaded,'totalBytes':total}))
        emit(output,'result',result)
    finally: session.close()

def main():
    if sys.argv[1:]==['--version']:
        print('chendownloader-tiktok-core '+VERSION+' protocol=1')
        return
    try: run(read_request(sys.stdin.buffer),sys.stdout)
    except (ValueError,OSError) as error:
        known={'content_changed','invalid_metadata','format_unavailable','unsupported_media'}
        code=error.code if isinstance(error,RequestError) else str(error) if isinstance(error,ValueError) and str(error) in known else 'invalid_metadata'
        emit(sys.stdout,'error',{'code':code})
        sys.exit(1)
    except Exception:
        emit(sys.stdout,'error',{'code':'process_failed'})
        sys.exit(1)

if __name__=='__main__': main()
