"""Anonymous, bounded TikTok transport. Cookies remain inside native protocol messages."""
import time
import re
from urllib.parse import urljoin,urlsplit
from curl_cffi import requests

from protocol import RequestError
from url_policy import PAGE_HOSTS,asset_url,parsed_url

UA='Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36'

class TikTokSession:
    def __init__(self,network,context=None,transport=None,seconds=120):
        self.network=network
        self.deadline=time.monotonic()+seconds
        self.client=transport or requests.Session(impersonate='chrome146',headers={'User-Agent':UA},trust_env=False,verify=True)
        self.request_headers={}
        self.request_host=None
        if context:
            headers=context.get('headers',{})
            if not isinstance(headers,dict) or len(headers)>16 or sum(len(str(k))+len(str(v)) for k,v in headers.items())>16384: raise RequestError()
            allowed={'user-agent':'User-Agent','referer':'Referer','origin':'Origin','accept':'Accept','accept-language':'Accept-Language','cookie':'Cookie','authorization':'Authorization'}
            for name,value in headers.items():
                if not isinstance(name,str) or not re.fullmatch(r"[!#$%&'*+.^_`|~0-9A-Za-z-]{1,64}",name) or name.lower() in ('host','content-length','transfer-encoding','connection','upgrade','te','range') or name.lower().startswith('proxy-') or not isinstance(value,str) or any(ord(c)<32 or ord(c)>126 for c in value): raise RequestError()
                self.request_headers[allowed.get(name.lower(),name)]=value
            host=context.get('requestHost')
            if host is not None:
                if not isinstance(host,str): raise RequestError()
                asset_url('https://'+host+'/')
                self.request_host=host.lower()
        if context and transport is None:
            from http.cookies import SimpleCookie
            encoded=context.get('serializedCookies','')
            if not isinstance(encoded,str) or len(encoded)>16384 or any(ord(c)<32 or ord(c)>126 for c in encoded):raise RequestError()
            parsed=SimpleCookie();parsed.load(encoded)
            if len(parsed)>64:raise RequestError()
            for name,cookie in parsed.items():
                if any(ord(c)<32 or ord(c)>126 for c in cookie.value):raise RequestError()
                domain=cookie['domain'].lstrip('.').lower() or self.request_host
                if not domain or not self.request_host or not (self.request_host==domain or self.request_host.endswith('.'+domain)):continue
                path=cookie['path'] or '/'
                if not path.startswith('/') or len(path)>1024:raise RequestError()
                self.client.cookies.set(name,cookie.value,domain=cookie['domain'] or domain,path=path)
            cookies=context.get('cookies',[])
            if not isinstance(cookies,list) or len(cookies)>64: raise RequestError()
            if sum(len(str(c)) for c in cookies)>16384: raise RequestError()
            for cookie in cookies:
                domain=cookie.get('domain','').lstrip('.')
                name=cookie.get('name'); value=cookie.get('value')
                if domain not in ('tiktok.com','www.tiktok.com') or not isinstance(name,str) or not isinstance(value,str): raise RequestError()
                if any(ord(c)<32 or c in '\r\n;' for c in name+value): raise RequestError()
                self.client.cookies.set(name,value,domain=cookie['domain'],path='/')

    def close(self):
        self.client.close()

    def context(self):
        cookies=[]
        for cookie in self.client.cookies.jar:
            if cookie.domain.lstrip('.') in ('tiktok.com','www.tiktok.com'):
                cookies.append({'name':cookie.name,'value':cookie.value,'domain':cookie.domain})
        if sum(len(str(c)) for c in cookies)>16384: raise RequestError('invalid_metadata')
        return {'cookies':cookies}

    def proxy(self,host):
        for rule in self.network.get('bypass',[]):
            domain=rule.lstrip('.').lower()
            if rule=='*' or host==domain or host.endswith('.'+domain): return ''
        return self.network.get('proxy') or ''

    def _validate(self,url,kind):
        if kind=='asset': asset_url(url)
        else:
            parsed=parsed_url(url)
            if parsed.hostname not in PAGE_HOSTS: raise ValueError('unsafe_page_host')

    def open(self,url,kind='page',headers=None):
        for _ in range(6):
            self._validate(url,kind)
            remaining=self.deadline-time.monotonic()
            if remaining<=0: raise RequestError('analysis_timeout')
            response=None
            request_headers=dict(headers or {})
            request_headers.update(self.request_headers)
            if urlsplit(url).hostname!=self.request_host:
                public={'user-agent','referer','origin','accept','accept-language','sec-fetch-mode','sec-fetch-site','sec-fetch-dest','sec-ch-ua','sec-ch-ua-mobile','sec-ch-ua-platform'}
                for name in self.request_headers:
                    if name.lower() not in public:request_headers.pop(name,None)
            for attempt in range(3):
                try:
                    response=self.client.get(url,stream=True,allow_redirects=False,verify=True,
                                             proxy=self.proxy(urlsplit(url).hostname),
                                             timeout=(min(10,remaining),min(20,remaining)),headers=request_headers)
                except requests.exceptions.RequestException:
                    if attempt==2: raise RequestError('network_failed') from None
                    continue
                if response.status_code not in (429,502,503,504): break
                status=response.status_code
                retry=response.headers.get('retry-after','1')
                response.close()
                if attempt==2: raise RequestError('rate_limited' if status==429 else 'network_failed')
                delay=float(retry) if retry.isdigit() else 1
                if delay>=self.deadline-time.monotonic(): raise RequestError('rate_limited')
                time.sleep(delay)
            if response.status_code in (301,302,303,307,308):
                location=response.headers.get('location','')
                response.close()
                if not location: raise RequestError('content_unavailable')
                url=urljoin(url,location)
                if urlsplit(url).path.startswith('/login'): raise RequestError('authentication_required')
                continue
            if response.status_code>=400:
                code={401:'authentication_required',403:'access_denied',404:'content_unavailable',410:'content_unavailable'}.get(response.status_code,'network_failed')
                response.close()
                raise RequestError(code)
            return response,url
        raise RequestError('invalid_url')

    def read(self,url,kind='page',limit=8*1024*1024,headers=None):
        response,final_url=self.open(url,kind,headers)
        try:
            data=bytearray()
            for chunk in response.iter_content(chunk_size=65536):
                if time.monotonic()>self.deadline: raise RequestError('analysis_timeout')
                if len(data)+len(chunk)>limit: raise RequestError('invalid_metadata')
                data.extend(chunk)
            return bytes(data),final_url,dict(response.headers)
        finally: response.close()
