import os

from panther import Panther
from panther.app import API
from panther.request import Request


@API()
async def api(request: Request):
    return {
        'instance': os.getenv('PINGATE_INSTANCE', 'upstream'),
        'method': request.method,
        'query_params': request.query_params,
        'headers': request.headers.__dict__,
    }


app = Panther(__name__, configs=__name__, urls={'/': api})
