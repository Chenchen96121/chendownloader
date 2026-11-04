"""Bounded private native protocol. Error messages intentionally contain no request data."""
import json
from urllib.parse import urlsplit

VERSION='1.1.0'
PROTOCOL=1
MAX_INPUT=65536
MAX_OUTPUT=4*1024*1024

class RequestError(ValueError):
    def __init__(self,code='invalid_request'):
        super().__init__(code)
        self.code=code

def read_request(stream):
    raw=stream.read(MAX_INPUT+1)
    if len(raw)>MAX_INPUT: raise RequestError()
    try: value=json.loads(raw)
    except (ValueError,UnicodeError): raise RequestError() from None
    allowed={'protocolVersion','operation','url','network','context','asset','stagingPath','sourceIdentity'}
    if not isinstance(value,dict) or set(value)-allowed or type(value.get('protocolVersion')) is not int or value['protocolVersion']!=PROTOCOL:
        raise RequestError()
    if value.get('operation') not in ('analyze','preview','download','probe'): raise RequestError()
    if not isinstance(value.get('url'),str) or not isinstance(value.get('network'),dict): raise RequestError()
    network=value['network']
    if set(network)-{'proxy','bypass'} or not isinstance(network.get('bypass',[]),list): raise RequestError()
    proxy=network.get('proxy')
    if proxy is not None:
        if not isinstance(proxy,str) or len(proxy)>4096: raise RequestError()
        try: parsed=urlsplit(proxy)
        except ValueError: raise RequestError() from None
        if parsed.scheme not in ('http','https') or not parsed.hostname or parsed.username or parsed.password or parsed.path not in ('','/') or parsed.query or parsed.fragment:
            raise RequestError()
    if any(not isinstance(x,str) or len(x)>253 for x in network.get('bypass',[])): raise RequestError()
    if value['operation']!='analyze' and (not isinstance(value.get('asset'),dict) or not isinstance(value.get('context'),dict)):
        raise RequestError()
    return value

def emit(stream,kind,data):
    value={'protocolVersion':PROTOCOL,'type':kind,'data':data}
    raw=json.dumps(value,ensure_ascii=True,separators=(',',':'))
    if len(raw.encode())>MAX_OUTPUT: raise RequestError('invalid_metadata')
    stream.write(raw+'\n')
    stream.flush()
