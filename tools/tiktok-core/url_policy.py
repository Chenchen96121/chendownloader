"""TikTok page/CDN allowlists: never an arbitrary URL fetcher."""
import re
from urllib.parse import urlsplit,urlunsplit

PAGE_HOSTS={'tiktok.com','www.tiktok.com','vm.tiktok.com','vt.tiktok.com'}
CDN_SUFFIXES=('tiktokcdn.com','tiktokcdn-us.com','tiktokcdn-eu.com','tiktokv.com','byteoversea.com','ibytedtos.com','muscdn.com')
VIDEO_HOSTS={'v16-webapp-prime.tiktok.com','v19-webapp-prime.tiktok.com'}
POST=re.compile(r'/@([\w.-]{1,64})/(video|photo)/([0-9]{1,20})/?$',re.ASCII)

def parsed_url(value):
    if not isinstance(value,str) or len(value)>8192 or '\\' in value or any(ord(c)<33 for c in value): raise ValueError('invalid_url')
    try: p=urlsplit(value); port=p.port
    except ValueError: raise ValueError('invalid_url') from None
    if p.scheme!='https' or not p.hostname or p.username is not None or p.password is not None or port not in (None,443): raise ValueError('invalid_url')
    if any(part in ('.','..') for part in p.path.split('/')): raise ValueError('invalid_url')
    return p

def normalize_url(text):
    if not isinstance(text,str) or len(text)>8192 or '\\' in text or any(ord(c)<32 and c not in '\t\r\n' for c in text): raise ValueError('invalid_url')
    urls=re.findall(r'https?://[^\s<>"“”]+',text)
    if len(urls)!=1: raise ValueError('invalid_url')
    candidate=urls[0].rstrip('。，,;；')
    p=parsed_url(candidate)
    if p.hostname not in PAGE_HOSTS: raise ValueError('unsupported_platform')
    if p.hostname in ('vm.tiktok.com','vt.tiktok.com') and re.fullmatch(r'/[A-Za-z0-9]+/?',p.path):
        return urlunsplit(('https',p.hostname,p.path,'',''))
    if p.hostname in ('tiktok.com','www.tiktok.com'):
        if POST.fullmatch(p.path): return urlunsplit(('https','www.tiktok.com',p.path.rstrip('/'),'',''))
        if re.fullmatch(r'/t/[A-Za-z0-9]+/?',p.path): return urlunsplit(('https','www.tiktok.com',p.path,'',''))
    raise ValueError('unsupported_media')

def asset_url(value):
    p=parsed_url(value)
    host=p.hostname.lower()
    if host not in VIDEO_HOSTS and not any(host==d or host.endswith('.'+d) for d in CDN_SUFFIXES): raise ValueError('unsafe_media_host')
    return value
