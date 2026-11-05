import requests


def fetch(url, params=None):
    r = requests.get(url)
    r2 = requests.get(url, params=params)  # comment
    r3 = requests.get()
    r4 = requests.get(
        url,
        headers={"a": "b"},  # keep me
    )
    r5 = requests.get(url, timeout=5)
    return r, r2, r3, r4, r5


class Client:
    def __init__(self, opts=None, headers=None, x=None):
        if opts is None:
            opts = {}
        if headers is None:
            # build default headers
            headers = dict(
                accept="json",
                agent="semcode",
            )
        if x is None:
            y = 1
        self.opts = opts


def other(cfg=None):
    cfg = load_default() if cfg is None else cfg
    return cfg
