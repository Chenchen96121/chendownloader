"""Normalize direct metadata while keeping private URLs out of public application DTOs."""
import hashlib
import json
from urllib.parse import urlsplit
from url_policy import asset_url

def _urls(values):
    if not isinstance(values,list): return []
    result=[]
    for value in values:
        if not isinstance(value,str): continue
        try: asset_url(value)
        except ValueError: continue
        if value not in result: result.append(value)
        if len(result)==3: break
    return result

def _dimension(value):
    return value if type(value) is int and 1<=value<=16384 else None

def _id(post,identity):
    return post+'-'+hashlib.sha256(identity.encode()).hexdigest()[:20]

def normalize_detail(data,post_id):
    if not isinstance(data,dict) or str(data.get('id',''))!=post_id: raise ValueError('content_changed')
    images=(data.get('imagePost') or {}).get('images') or []
    if not isinstance(images,list) or len(images)>100: raise ValueError('invalid_metadata')
    items=[]
    if images:
        occurrences={}
        for ordinal,image in enumerate(images,1):
            address=image.get('imageURL') or {}
            urls=_urls(address.get('urlList'))
            if not urls: raise ValueError('format_unavailable')
            identity=address.get('uri') or urlsplit(urls[0]).path
            occurrences[identity]=occurrences.get(identity,0)+1
            items.append({'itemId':_id(post_id,identity+':'+str(occurrences[identity])),
                          'ordinal':ordinal,'kind':'image','identity':identity,
                          'width':_dimension(image.get('imageWidth')),'height':_dimension(image.get('imageHeight')),
                          'container':'jpg','urls':urls,'previewUrls':urls,'candidates':[]})
    else:
        video=data.get('video') or {}
        candidates=[]
        for source in video.get('bitrateInfo') or []:
            address=source.get('PlayAddr') or {}
            urls=_urls(address.get('UrlList'))
            width=_dimension(address.get('Width')); height=_dimension(address.get('Height'))
            codec={'h265_hvc1':'hevc','h265':'hevc','bytevc1':'hevc','h264':'h264','av1':'av1'}.get(source.get('CodecType'))
            if not urls or not width or not height: raise ValueError('invalid_metadata')
            if not codec: raise ValueError('format_unavailable')
            size=address.get('DataSize')
            fps=source.get('BitrateFPS',source.get('FPS'))
            try: extra=json.loads(source.get('VideoExtra') or '{}')
            except (ValueError,TypeError): extra={}
            audio_rate=extra.get('audio_bit_rate') if isinstance(extra,dict) else None
            # TikTok Web MP4 full-stream declaration, also used by the upstream extractor.
            # This is source metadata; the native processor must probe real tracks before publishing.
            full_audio=source.get('Format')=='mp4' and type(audio_rate) is int and audio_rate>0 and not any('/media-video-hvc1/' in u for u in urls)
            candidates.append({'identity':str(address.get('Uri') or address.get('UrlKey') or urlsplit(urls[0]).path),
                               'urls':urls,'width':width,'height':height,'videoCodec':codec,'container':'mp4',
                               'fileSize':{'bytes':size,'isApproximate':False} if type(size) is int and 0<size<2**53 else None,
                               'fps':fps if type(fps) in (int,float) and 0<fps<=240 else None,
                               'audioCodec':'aac' if full_audio else None,
                               'audioBitrateKbps':audio_rate/1000 if full_audio else None,
                               'videoBitrateKbps':source.get('Bitrate',0)/1000 if type(source.get('Bitrate')) is int else None})
        if not candidates: raise ValueError('format_unavailable')
        # Same-resolution codec tradeoffs are resolved with native stream probing/selection;
        # a lower-resolution high-bitrate H.264 stream must never replace a larger original.
        candidates.sort(key=lambda c:(c['width']*c['height'],c['fps'] or 0,c['videoCodec']=='av1',c['videoCodec']=='hevc',c['videoBitrateKbps'] or 0),reverse=True)
        best=candidates[0]
        covers=[]
        for key in ('originCover','cover','dynamicCover'):
            value=video.get(key)
            covers+=_urls(value if isinstance(value,list) else [value] if isinstance(value,str) else [])
        items=[dict(best,itemId=_id(post_id,'video'),ordinal=1,kind='video',previewUrls=covers[:3],candidates=candidates)]
    title=data.get('desc')
    if not isinstance(title,str) or not title.strip(): title='TikTok '+post_id
    author=(data.get('author') or {}).get('uniqueId')
    return {'postId':post_id,'title':title.strip()[:512],'creator':author if isinstance(author,str) else None,
            'duration':(data.get('video') or {}).get('duration'),'items':items}
