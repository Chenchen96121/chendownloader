"""Browser query encoder excerpt, adapted from Evil0ctal; Apache-2.0."""
from collections.abc import Iterable, Mapping
from types import MappingProxyType

_BROWSER_MUST_ESCAPE: Mapping[str, str] = MappingProxyType(
    {" ": "%20", '"': "%22", "<": "%3C", ">": "%3E", "`": "%60", "#": "%23"}
)


def encode_browser_query(pairs: Iterable[tuple[str, str]]) -> str:
    """Serialize query parameters exactly as a browser would send them.

    This is NOT ``urlencode``, and the difference is the whole reason
    ``/api/user/detail/`` used to fail while its neighbours worked.

    TikTok's X-Dynosaur field 0x2E is a hash of the query, and the SDK hashes
    the string the browser handed it - which is the URL after the browser's own
    normalisation, not after a library's. Chrome escapes a space to ``%20`` and
    leaves parentheses, slashes and colons alone, so the real page hashes
    ``browser_version=5.0%20(Windows)&root_referer=https://www.tiktok.com/``.
    Percent-encoding everything produced ``5.0%20%28Windows%29`` and
    ``https%3A%2F%2F...`` - a different string, a different hash, and a
    signature bound to bytes the platform never sees.

    Recovered by capturing a browser-signed request, decoding its X-Dynosaur and
    searching for the input that reproduces 0x2E: only-escape-the-space matches
    byte for byte, and nothing else tried does. TikTok verifies that binding on
    ``/api/user/detail/`` and ``/api/post/item_list/`` and ignores it on
    ``/api/item/detail/`` and ``/api/comment/list/``, which is why the symptom
    is one endpoint silently returning nothing.

    The same string is hashed and sent, so this must stay the only encoder in
    the path.
    """
    return "&".join(f"{_browser_escape(key)}={_browser_escape(value)}" for key, value in pairs)


def _browser_escape(text: str) -> str:
    """Percent-escape only what a browser has to, leaving the rest literal.

    Printable ASCII passes through untouched apart from the handful in
    :data:`_BROWSER_MUST_ESCAPE`. Everything else - control characters and
    anything non-ASCII - is percent-encoded from its UTF-8 bytes, which is what
    a browser puts on the wire for a query it was handed as text.
    """
    out: list[str] = []
    for ch in text:
        if ch in _BROWSER_MUST_ESCAPE:
            out.append(_BROWSER_MUST_ESCAPE[ch])
        elif " " < ch <= "~":
            out.append(ch)
        else:
            out.extend(f"%{byte:02X}" for byte in ch.encode("utf-8"))
    return "".join(out)
