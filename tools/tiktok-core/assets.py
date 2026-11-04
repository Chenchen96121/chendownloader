"""Stream originals to a new native-owned staging file, never directly to user output."""
import base64
import errno
import os
from pathlib import Path
import time

from protocol import RequestError

def image_type(data):
    if data.startswith(b'\xff\xd8\xff'): return 'image/jpeg'
    if data.startswith(b'\x89PNG\r\n\x1a\n'): return 'image/png'
    if data[:4]==b'RIFF' and data[8:12]==b'WEBP': return 'image/webp'
    if data[4:8]==b'ftyp' and b'avif' in data[:32]: return 'image/avif'
    raise RequestError('image_content_invalid')

def preview(session,asset,source):
    for url in asset.get('previewUrls',[])[:3]:
        try:
            data,_,_=session.read(url,kind='asset',limit=8*1024*1024,headers={'Referer':source})
            return {'mimeType':image_type(data),'dataBase64':base64.b64encode(data).decode('ascii')}
        except (ValueError,OSError): continue
    raise RequestError('content_unavailable')

def probe_file(session,asset,path,source):
    """A disposable initial range for metadata only, never an output media file."""
    target=Path(path)
    if not target.is_absolute() or target.suffix!='.part' or not target.parent.is_dir(): raise RequestError()
    for component in [target,*target.parents]:
        if component.is_symlink() or (hasattr(component,'is_junction') and component.is_junction()): raise RequestError()
    limit=1024*1024
    for url in asset.get('urls',[])[:3]:
        created=False;response=None
        try:
            response,_=session.open(url,kind='asset',headers={'Referer':source,'Range':f'bytes=0-{limit-1}'})
            written=0
            with target.open('xb') as handle:
                created=True
                for chunk in response.iter_content(chunk_size=65536):
                    if time.monotonic()>session.deadline: raise RequestError('analysis_timeout')
                    chunk=chunk[:limit-written];handle.write(chunk);written+=len(chunk)
                    if written>=limit:break
            with target.open('rb') as handle:magic=handle.read(32)
            if written<16 or magic[4:8]!=b'ftyp':raise RequestError('invalid_metadata')
            return {'bytes':written,'isPartial':True}
        except (ValueError,OSError):
            if created:target.unlink(missing_ok=True)
        finally:
            if response is not None:response.close()
    raise RequestError('format_unavailable')

def stream_file(session,asset,path,source,progress):
    target=Path(path)
    if not target.is_absolute() or target.suffix!='.part' or not target.parent.is_dir(): raise RequestError()
    for component in [target,*target.parents]:
        if component.is_symlink() or (hasattr(component,'is_junction') and component.is_junction()): raise RequestError()
    last='network_failed'
    for url in asset.get('urls',[])[:3]:
        created=False
        response=None
        try:
            response,_=session.open(url,kind='asset',headers={'Referer':source})
            header=response.headers.get('content-length','')
            total=int(header) if header.isdigit() else None
            maximum=asset.get('maximumBytes',9_007_199_254_740_991)
            if type(maximum) is not int or maximum<1 or maximum>9_007_199_254_740_991: raise RequestError()
            if total is not None and total>maximum: raise RequestError('output_invalid')
            downloaded=0
            last_progress=0.0
            with target.open('xb') as handle:
                created=True
                for chunk in response.iter_content(chunk_size=65536):
                    if time.monotonic()>session.deadline: raise RequestError('download_timeout')
                    handle.write(chunk)
                    downloaded+=len(chunk)
                    if downloaded>maximum: raise RequestError('output_invalid')
                    if time.monotonic()-last_progress>=0.25 or downloaded==total:
                        progress(downloaded,total)
                        last_progress=time.monotonic()
            if not downloaded or (total is not None and downloaded!=total): raise RequestError('output_invalid')
            expected=(asset.get('fileSize') or {}).get('bytes')
            if expected and not (asset.get('fileSize') or {}).get('isApproximate') and expected!=downloaded: raise RequestError('output_invalid')
            with target.open('rb') as handle: magic=handle.read(32)
            if asset.get('kind')=='image': mime=image_type(magic)
            elif magic[4:8]==b'ftyp': mime='video/mp4'
            else: raise RequestError('output_invalid')
            return {'path':str(target),'bytes':downloaded,'mimeType':mime}
        except (ValueError,OSError) as error:
            last=error.code if isinstance(error,RequestError) else 'disk_full' if isinstance(error,OSError) and error.errno==errno.ENOSPC else 'network_failed'
            if created: target.unlink(missing_ok=True)
            if last in ('disk_full','download_timeout'): break
        finally:
            if response is not None: response.close()
    raise RequestError(last)
