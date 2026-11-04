import importlib
import io
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))

class FakeResponse:
    def __init__(self,status,body=b'',headers=None):
        self.status_code=status; self.body=body; self.headers=headers or {}; self.closed=False
    def iter_content(self,chunk_size):
        for i in range(0,len(self.body),chunk_size): yield self.body[i:i+chunk_size]
    def close(self): self.closed=True

class FakeSession:
    def __init__(self,responses): self.responses=iter(responses); self.calls=[]
    def get(self,url,**kwargs): self.calls.append((url,kwargs)); return next(self.responses)

class TransportTests(unittest.TestCase):
    def test_retry_after_seventy_seconds_is_allowed_inside_analysis_deadline(self):
        from unittest.mock import patch
        import session as transport
        clock=[10.0];delays=[]
        def sleep(seconds):delays.append(seconds);clock[0]+=seconds
        fake=FakeSession([FakeResponse(429,headers={'retry-after':'70'}),FakeResponse(200,b'ok')])
        with patch.object(transport.time,'monotonic',lambda:clock[0]),patch.object(transport.time,'sleep',sleep):
            client=transport.TikTokSession({'proxy':None,'bypass':[]},transport=fake,seconds=120)
            self.assertEqual(client.read('https://www.tiktok.com/@public/video/123')[0],b'ok')
        self.assertEqual(delays,[70.0])

    def test_extractor_headers_follow_only_their_own_media_request_context(self):
        from session import TikTokSession
        fake=FakeSession([FakeResponse(302,headers={'location':'https://v19-webapp-prime.tiktok.com/file'}),FakeResponse(200,b'ok')])
        context={'cookies':[],'headers':{'User-Agent':'extractor-agent','Referer':'https://www.tiktok.com/@public/video/123','Cookie':'anonymous=value'},'requestHost':'v16-webapp-prime.tiktok.com'}
        session=TikTokSession({'proxy':None,'bypass':[]},context,transport=fake)
        session.read('https://v16-webapp-prime.tiktok.com/file',kind='asset',headers={'Referer':'https://www.tiktok.com/'})
        self.assertEqual(fake.calls[0][1]['headers']['User-Agent'],'extractor-agent')
        self.assertEqual(fake.calls[0][1]['headers']['Referer'],context['headers']['Referer'])
        self.assertEqual(fake.calls[0][1]['headers']['Cookie'],'anonymous=value')
        self.assertNotIn('Cookie',fake.calls[1][1]['headers'])

    def test_short_link_redirect_resolves_only_to_valid_canonical_post(self):
        from url_policy import normalize_url
        target='https://www.tiktok.com/@public/video/123?tracking=1'
        session,fake=self.transport([FakeResponse(302,headers={'location':target}),FakeResponse(200,b'page')])
        body,final,_=session.read('https://vm.tiktok.com/ABC/',kind='page')
        self.assertEqual(body,b'page')
        self.assertEqual(normalize_url(final),'https://www.tiktok.com/@public/video/123')
        self.assertEqual(len(fake.calls),2)
        for destination in ['https://www.tiktok.com/','https://www.tiktok.com/login']:
            session,_=self.transport([FakeResponse(302,headers={'location':destination}),FakeResponse(200,b'page')])
            with self.assertRaises(ValueError):
                _,final,_=session.read('https://vm.tiktok.com/ABC/',kind='page')
                normalize_url(final)

    def test_direct_id_still_tries_public_detail_after_page_login_redirect(self):
        from main import analyze
        from protocol import RequestError
        from types import SimpleNamespace
        from test_core import video
        class PageBlocked:
            client=SimpleNamespace(cookies=SimpleNamespace(jar=[]))
            def read(self,url,**kwargs):
                if '/api/item/detail/' not in url: raise RequestError('authentication_required')
                return json.dumps({'itemInfo':{'itemStruct':video()}}).encode(),url,{}
            def context(self): return {'cookies':[]}
        result=analyze(PageBlocked(),'https://www.tiktok.com/@public/video/1234567890123456789')
        self.assertEqual(result['postId'],'1234567890123456789')

    def transport(self,responses):
        self.assertIsNotNone(importlib.util.find_spec('session'),'session implementation is missing')
        module=importlib.import_module('session')
        fake=FakeSession(responses)
        return module.TikTokSession({'proxy':None,'bypass':[]},transport=fake),fake

    def test_validate_redirect_before_second_request(self):
        session,fake=self.transport([FakeResponse(302,headers={'location':'https://127.0.0.1/private'})])
        with self.assertRaises(ValueError): session.read('https://v16.tiktokcdn.com/file',kind='asset',limit=100)
        self.assertEqual(len(fake.calls),1)

    def test_response_limit_and_tls_always_enabled(self):
        session,fake=self.transport([FakeResponse(200,b'A'*101)])
        with self.assertRaises(ValueError): session.read('https://v16.tiktokcdn.com/file',kind='asset',limit=100)
        self.assertTrue(fake.calls[0][1]['verify'])
        self.assertFalse(fake.calls[0][1]['allow_redirects'])

    def test_proxy_bypass_is_per_host_not_global_environment(self):
        session,fake=self.transport([FakeResponse(200,b'ok')])
        session.network={'proxy':'http://127.0.0.1:7897','bypass':['.tiktokcdn.com']}
        self.assertEqual(session.read('https://v16.tiktokcdn.com/file',kind='asset',limit=100)[0],b'ok')
        self.assertEqual(fake.calls[0][1]['proxy'],'')

if __name__=='__main__': unittest.main()
