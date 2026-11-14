"""Original journal-before-send subclass with unchanged kernel-peer checks."""
import hashlib,json
from common import Public

class JournaledPublic(Public):
    """Keep common's original peer checks; journal every wire frame before send.

    This also covers the constructor hello and safe read-only polling calls.
    It never retries a failed send or receive.
    """
    def __init__(self,path,owned,record):
        self.record_wire=record
        super().__init__(path,owned,('command.receipts',))
    def request(self,method,params):
        self.counter+=1
        value={'id':self.counter,'method':method,'params':params}
        raw=json.dumps(value).encode()+b'\n'
        self.record_wire('public-wire-before-send',{'frame':value,'wireSHA256':hashlib.sha256(raw).hexdigest(),
            'attempts':1,'connectionCounter':self.counter})
        self.socket.sendall(raw)
        return self.counter

