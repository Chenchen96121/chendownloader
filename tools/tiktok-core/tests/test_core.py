import importlib
import importlib.util
import io
import json
from pathlib import Path
import random
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

def module(test, name):
    test.assertIsNotNone(importlib.util.find_spec(name), f'{name} implementation is missing')
    return importlib.import_module(name)

def video():
    return {'id':'1234567890123456789','desc':'Public video','author':{'uniqueId':'public'},
            'video':{'duration':12,'width':720,'height':1280,'bitrateInfo':[
                {'Bitrate':2000000,'CodecType':'h264','PlayAddr':{'Width':720,'Height':1280,'DataSize':2000,'Uri':'small','UrlList':['https://v16.tiktokcdn.com/small']}},
                {'Bitrate':1300000,'CodecType':'h265_hvc1','PlayAddr':{'Width':1080,'Height':1920,'DataSize':2100,'Uri':'large','UrlList':['https://v16.tiktokcdn.com/large']}}]}}

class SignerTests(unittest.TestCase):
    def test_signature_seals_exact_browser_query_and_ua(self):
        signer=module(self,'signer')
        query, parameters=signer.sign([('aid','1988'),('browser_version','5.0 (Windows)'),('q','中')],
                                    'test-UA',ms_token='',timestamp=1730000000,nonce=123,nonce2=456,
                                    rng=random.Random(1))
        self.assertTrue(query.startswith('aid=1988&browser_version=5.0%20(Windows)&q=%E4%B8%AD&X-Dynosaur='))
        self.assertEqual(parameters['X-Bogus'],'1')
        self.assertEqual(parameters['msToken'],'')
        self.assertLess(query.index('X-Dynosaur='),query.index('msToken='))
        self.assertLess(query.index('msToken='),query.index('X-Bogus='))
        self.assertLess(query.index('X-Bogus='),query.index('X-Gnarly='))
        self.assertNotEqual(query,signer.sign([('aid','1988')],'different-UA',timestamp=1730000000,
                                             nonce=123,nonce2=456,rng=random.Random(1))[0])

class ProtocolTests(unittest.TestCase):
    def test_valid_request_and_identity(self):
        protocol=module(self,'protocol')
        value={'protocolVersion':1,'operation':'analyze','url':'https://www.tiktok.com/@public/video/1234567890123456789','network':{'proxy':None,'bypass':[]}}
        self.assertEqual(protocol.read_request(io.BytesIO(json.dumps(value).encode())),value)
        self.assertEqual(protocol.VERSION,'1.1.0')

    def test_invalid_protocol_unknown_operation_extra_fields_and_oversize(self):
        protocol=module(self,'protocol')
        values=[{'protocolVersion':2,'operation':'analyze'}, {'protocolVersion':1,'operation':'execute'},
                {'protocolVersion':1,'operation':'analyze','url':'x','network':{},'cookiesFile':'secret'},
                {'protocolVersion':True,'operation':'analyze'}]
        for value in values:
            with self.subTest(value=value),self.assertRaises(protocol.RequestError):
                protocol.read_request(io.BytesIO(json.dumps(value).encode()))
        with self.assertRaises(protocol.RequestError):
            protocol.read_request(io.BytesIO(b' '*65537))

class UrlTests(unittest.TestCase):
    def test_canonical_video_photo_and_share_text(self):
        policy=module(self,'url_policy')
        self.assertEqual(policy.normalize_url('title https://www.tiktok.com/@public/photo/1234567890123456789?web_id=123 '),
                         'https://www.tiktok.com/@public/photo/1234567890123456789')
        self.assertEqual(policy.normalize_url('https://vm.tiktok.com/Abcd/'),'https://vm.tiktok.com/Abcd/')

    def test_source_and_asset_url_boundaries(self):
        policy=module(self,'url_policy')
        for bad in ['http://www.tiktok.com/@public/video/1','https://evil.test/@public/video/1',
                    'https://www.tiktok.com.evil.test/@public/video/1','https://user@www.tiktok.com/@public/video/1',
                    'https://www.tiktok.com:444/@public/video/1','https://www.tiktok.com/@public',
                    'https://www.tiktok.com/@public/video/%31','https://www.tiktok.com/../@public/video/1',
                    'https://www.tiktok.com/@public/video/1 https://www.tiktok.com/@public/video/2']:
            with self.subTest(url=bad),self.assertRaises(ValueError): policy.normalize_url(bad)
        for bad in ['http://v16.tiktokcdn.com/video','https://127.0.0.1/video','https://localhost/video',
                    'https://tiktokcdn.com.evil.test/video','https://v16.tiktokcdn.com:444/video']:
            with self.subTest(url=bad),self.assertRaises(ValueError): policy.asset_url(bad)
        self.assertEqual(policy.asset_url('https://v16.tiktokcdn.com/video?a=1'),'https://v16.tiktokcdn.com/video?a=1')
        self.assertEqual(policy.asset_url('https://v16-webapp-prime.tiktok.com/video'),'https://v16-webapp-prime.tiktok.com/video')
        with self.assertRaises(ValueError): policy.asset_url('https://www.tiktok.com/api/item/detail/')

class DetailTests(unittest.TestCase):
    def test_unknown_high_quality_codec_never_silently_chooses_a_lower_stream(self):
        detail=module(self,'detail')
        data=video()
        data['video']['bitrateInfo'][1]['CodecType']='future-codec'
        with self.assertRaises(ValueError): detail.normalize_detail(data,data['id'])

    def test_reads_declared_frame_rate_and_full_stream_audio_metadata(self):
        detail=module(self,'detail')
        data=video()
        for row in data['video']['bitrateInfo']:
            row.update(BitrateFPS=30,Format='mp4',VideoExtra=json.dumps({'audio_bit_rate':64921}))
        item=detail.normalize_detail(data,data['id'])['items'][0]
        self.assertEqual(item['fps'],30)
        self.assertEqual(item['audioCodec'],'aac')
        self.assertAlmostEqual(item['audioBitrateKbps'],64.921)

    def test_highest_resolution_is_not_basic_video_dimensions_or_bare_bitrate(self):
        detail=module(self,'detail')
        result=detail.normalize_detail(video(),'1234567890123456789')
        self.assertEqual(len(result['items']),1)
        item=result['items'][0]
        self.assertEqual((item['width'],item['height']),(1080,1920))
        self.assertEqual(item['videoCodec'],'hevc')
        self.assertEqual(item['fileSize']['bytes'],2100)
        self.assertNotIn('music',item)
        self.assertEqual(len(item['candidates']),2)

    def test_photo_ordinals_stable_identity_and_original_image_addresses(self):
        detail=module(self,'detail')
        data={'id':'1234567890123456789','desc':'Photo post','imagePost':{'images':[
            {'imageWidth':2160,'imageHeight':2880,'imageURL':{'uri':f'original-{i}','urlList':[f'https://p16.tiktokcdn.com/original-{i}?sig=one']}}
            for i in range(13)]}}
        first=detail.normalize_detail(data,data['id'])
        self.assertEqual([i['ordinal'] for i in first['items']],list(range(1,14)))
        self.assertEqual(len({i['itemId'] for i in first['items']}),13)
        data['imagePost']['images'][0]['imageURL']['urlList'][0]='https://p16.tiktokcdn.com/original-0?sig=two'
        second=detail.normalize_detail(data,data['id'])
        self.assertEqual(first['items'][0]['itemId'],second['items'][0]['itemId'])
        self.assertEqual(first['items'][0]['width'],2160)

    def test_wrong_identity_empty_post_and_too_many_items_fail(self):
        detail=module(self,'detail')
        for data,identity in [(video(),'999'),({'id':'1'},'1'),
                              ({'id':'1','imagePost':{'images':[{}]*101}},'1')]:
            with self.subTest(identity=identity),self.assertRaises(ValueError):
                detail.normalize_detail(data,identity)

if __name__=='__main__': unittest.main()
