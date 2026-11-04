import importlib
from pathlib import Path
import sys
import tempfile
import time
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from assets import stream_file
from protocol import RequestError

class Response:
    def __init__(self,data,headers): self.data=data; self.headers=headers
    def iter_content(self,chunk_size): yield self.data
    def close(self): pass
class Session:
    deadline=float('inf')
    def __init__(self,data,headers): self.response=Response(data,headers)
    def open(self,*args,**kwargs): return self.response,'https://p16.tiktokcdn.com/original'

class AssetTests(unittest.TestCase):
    def test_quality_sample_is_bounded_even_when_server_ignores_range(self):
        import assets
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'sample.part'
            data=b'\0\0\0\x18ftypmp42'+b'A'*(2*1024*1024)
            result=assets.probe_file(Session(data,{}),{'urls':['https://v16-webapp-prime.tiktok.com/original']},str(path),'https://www.tiktok.com/@public/video/1')
            self.assertEqual(path.stat().st_size,1024*1024)
            self.assertTrue(result['isPartial'])
            with self.assertRaises(RequestError):assets.probe_file(Session(data,{}),{'urls':['https://v16-webapp-prime.tiktok.com/original']},str(path),'https://www.tiktok.com/@public/video/1')

    def test_html_and_truncated_original_never_leave_partial_files(self):
        for data,headers in [(b'<html>blocked</html>',{}),(b'\xff\xd8\xffdata',{'content-length':'100'})]:
            with tempfile.TemporaryDirectory() as root:
                path=Path(root)/'stage.part'
                with self.assertRaises(RequestError): stream_file(Session(data,headers),{'kind':'image','urls':['https://p16.tiktokcdn.com/original']},str(path),'https://www.tiktok.com/@public/photo/1',lambda *x:None)
                self.assertFalse(path.exists())

    def test_unknown_size_stream_preserves_bytes_and_existing_file_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'stage.part'; data=b'\xff\xd8\xfforiginal'
            result=stream_file(Session(data,{}),{'kind':'image','urls':['https://p16.tiktokcdn.com/original']},str(path),'https://www.tiktok.com/@public/photo/1',lambda *x:None)
            self.assertEqual(result['bytes'],len(data));self.assertEqual(path.read_bytes(),data)
            with self.assertRaises(RequestError): stream_file(Session(b'new',{}),{'kind':'image','urls':['https://p16.tiktokcdn.com/original']},str(path),'https://www.tiktok.com/@public/photo/1',lambda *x:None)
            self.assertEqual(path.read_bytes(),data)

    def test_preview_byte_budget_is_enforced_while_streaming(self):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'stage.part'
            with self.assertRaises(RequestError): stream_file(Session(b'\xff\xd8\xff'+b'A'*100,{}),{'kind':'image','urls':['https://p16.tiktokcdn.com/original'],'maximumBytes':64},str(path),'https://www.tiktok.com/@public/photo/1',lambda *x:None)
            self.assertFalse(path.exists())

if __name__=='__main__': unittest.main()
