"""Probe Unix permission and denied socket creation; never dial any IP host."""
import argparse,errno,json,os,socket,stat
from pathlib import Path


def main():
    p=argparse.ArgumentParser();p.add_argument('--mode',choices=['client','bind'],required=True)
    p.add_argument('--endpoint',type=Path,required=True);p.add_argument('--tcp-port',type=int,required=True);a=p.parse_args()
    if a.mode=='bind':
        assert not os.path.lexists(a.endpoint)
        with socket.socket(socket.AF_UNIX) as peer:
            peer.bind(str(a.endpoint));peer.listen(1);original=a.endpoint.lstat()
        now=a.endpoint.lstat();assert stat.S_ISSOCK(now.st_mode) and now.st_uid==os.getuid()
        assert (now.st_dev,now.st_ino)==(original.st_dev,original.st_ino);a.endpoint.unlink()
    else:
        with socket.socket(socket.AF_UNIX) as peer:peer.settimeout(2);peer.connect(str(a.endpoint))
    for family in [socket.AF_INET,socket.AF_INET6,socket.AF_NETLINK]:
        try:peer=socket.socket(family,socket.SOCK_STREAM)
        except OSError as error:assert error.errno in [errno.EPERM,errno.EACCES],error
        else:peer.close();raise AssertionError('non-Unix socket creation unexpectedly permitted')
    print(json.dumps({'mode':a.mode,'ownedUnixAllowed':True,'allNonUnixSocketsDenied':True,'externalDialAttempts':0}))


if __name__=='__main__':main()
