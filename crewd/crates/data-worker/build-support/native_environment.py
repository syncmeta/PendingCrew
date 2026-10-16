#!/usr/bin/python3
"""Closed outer-Cargo environment. Preparation only: this module executes no tool.

The parent qualification is independent of contract02's original finite ambient
map. Its complete receipt and reviewed config must be included in sourceInputs
and command-index snapshots by the owning actual build producer.
"""
import sys
sys.dont_write_bytecode = True
import hashlib,json,os,pathlib,stat

FORBIDDEN={
    'RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER',
    'RUSTC_BOOTSTRAP','CARGO_BUILD_RUSTFLAGS','CARGO_BUILD_RUSTC_WRAPPER','CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER',
    'RANLIB','CPATH','C_INCLUDE_PATH','CPLUS_INCLUDE_PATH','OBJC_INCLUDE_PATH',
    'LIBRARY_PATH','COMPILER_PATH','GCC_EXEC_PREFIX','LD_PRELOAD','LD_LIBRARY_PATH',
    'LD_AUDIT','LLVM_AR_LIBRARY_PATH','CCC_OVERRIDE_OPTIONS',
    'CLANG_CONFIG_FILE_USER_DIR','CLANG_CONFIG_FILE_SYSTEM_DIR','CC_FORCE_DISABLE',
    'CRATE_CC_NO_DEFAULTS','CARGO_ENCODED_RUSTDOCFLAGS','RUSTDOCFLAGS',
}
FLAG_FAMILIES=('CFLAGS','CXXFLAGS','CPPFLAGS','ARFLAGS','RANLIB')
INJECTION_PREFIXES=('DYLD_','CCC_','CLANG_CONFIG','CARGO_PROFILE_')
CLOSED_KEYS={'version','tools','cargoHome','rustupHome','targetDirectory','wrappers','nativeConfig','nativeConfigSHA256','recordRoot','sdkEnvironment'}

def require(ok,message):
    if not ok:raise ValueError(message)

def sha(value):return hashlib.sha256(value).hexdigest()
def canonical_directory(value,private=False):
    p=pathlib.Path(value);m=p.lstat()
    require(p.is_absolute() and p.resolve(strict=True)==p and stat.S_ISDIR(m.st_mode) and m.st_uid in (0,os.geteuid()) and not m.st_mode&0o022,'unqualified build directory')
    if private:require(m.st_uid==os.geteuid() and stat.S_IMODE(m.st_mode)==0o700,'unqualified private build directory')
    return str(p)
def qualified_file(proof,executable=False):
    require(set(proof)=={'path','sha256'},'qualified input fields')
    p=pathlib.Path(proof['path']);m=p.lstat()
    require(p.is_absolute() and p.resolve(strict=True)==p and stat.S_ISREG(m.st_mode) and m.st_uid in (0,os.geteuid()) and not m.st_mode&0o6022 and m.st_size<=512*1024*1024,'unqualified build input')
    if executable:require(m.st_mode&0o111,'nonexecutable build tool')
    fd=os.open(str(p),os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC|os.O_NONBLOCK)
    try:
        stamp=lambda x:(x.st_dev,x.st_ino,x.st_size,x.st_mtime_ns,x.st_ctime_ns,x.st_mode,x.st_uid)
        require(stamp(os.fstat(fd))==stamp(m),'build input changed before open')
        h=hashlib.sha256();n=0
        while True:
            part=os.read(fd,65536)
            if not part:break
            n+=len(part);require(n<=m.st_size,'build input grew');h.update(part)
        require(stamp(os.fstat(fd))==stamp(m) and stamp(p.lstat())==stamp(m) and n==m.st_size and h.hexdigest()==proof['sha256'],'build input bytes changed')
    finally:os.close(fd)
    return str(p)
def family(key,base):return key==base or key.startswith(base+'_') or key.endswith('_'+base)
def prepare(review,parent):
    require(set(review)==CLOSED_KEYS and review['version']==1,'build review shape')
    require(set(review['tools'])=={'cargo','rustc','rustdoc'} and set(review['wrappers'])=={'CC','AR'},'build role closure')
    paths={k:qualified_file(v,True) for k,v in review['tools'].items()}
    wrappers={k:qualified_file(v,True) for k,v in review['wrappers'].items()}
    record=canonical_directory(review['recordRoot'],True)
    native=qualified_file({'path':review['nativeConfig'],'sha256':review['nativeConfigSHA256']})
    sdk=review['sdkEnvironment'];require(set(sdk)<={'SDKROOT','DEVELOPER_DIR','MACOSX_DEPLOYMENT_TARGET'} and all(isinstance(v,str) and v for v in sdk.values()),'SDK review fields')
    for key in ('SDKROOT','DEVELOPER_DIR'):
        if key in sdk:canonical_directory(sdk[key])
    # Only key names and reviewed non-secret tool/SDK configuration are inspected.
    # No arbitrary parent value is copied into the Cargo or compiler child.
    require(len(parent)<=16384 and all(isinstance(k,str) and len(k)<=1024 for k in parent),'parent key bound')
    observed={};rejected=[]
    for key in sorted(parent):
        value=parent[key]
        deny=key in FORBIDDEN or key.startswith(INJECTION_PREFIXES) or any(family(key,b) for b in FLAG_FAMILIES) or (key.startswith('CARGO_TARGET_') and key.endswith(('_RUSTFLAGS','_LINKER','_RUNNER')))
        wrapper=(family(key,'CC') or family(key,'AR')) and key!='CC_ENABLE_DEBUG_OUTPUT'
        if deny or wrapper or key=='CC_ENABLE_DEBUG_OUTPUT':
            require(isinstance(value,str),'parent environment value type')
            # Receipt deliberately contains presence/empty state, never a
            # rejected flag value, compiler option or credential-like text.
            observed[key]={'present':True,'nonempty':bool(value)}
            if value and deny:rejected.append(key)
            elif value and wrapper:
                role='CC' if family(key,'CC') else 'AR'
                if value!=wrappers[role]:rejected.append(key)
            elif value and key=='CC_ENABLE_DEBUG_OUTPUT' and value!='1':rejected.append(key)
    for key in sorted(FORBIDDEN|set(FLAG_FAMILIES)|{'CC','AR','CC_ENABLE_DEBUG_OUTPUT'}):
        observed.setdefault(key,{'present':False,'nonempty':False})
    # Fail before any Cargo/compiler/archiver launch. The caller preserves these
    # exact key-only diagnostics; it must not silently sanitize then continue.
    require(not rejected,'ambient build injection refused: '+','.join(rejected))
    for key,value in sdk.items():
        require(not parent.get(key) or parent[key]==value,'ambient SDK differs from reviewed input: '+key)
    toolbin=str(pathlib.Path(paths['rustc']).parent)
    require(all(str(pathlib.Path(p).parent)==toolbin for p in paths.values()),'toolchain binaries must share one reviewed directory')
    environment={
        'PATH':toolbin+':/usr/bin:/bin','LANG':'C','LC_ALL':'C','TZ':'UTC',
        'TMPDIR':canonical_directory(str(pathlib.Path(record)/'tmp'),True),
        'CARGO_HOME':canonical_directory(review['cargoHome']),
        'RUSTUP_HOME':canonical_directory(review['rustupHome']),
        'CARGO_TARGET_DIR':canonical_directory(review['targetDirectory']),
        'CARGO_BUILD_JOBS':'1','CARGO_NET_OFFLINE':'true','CARGO_TERM_COLOR':'never',
        'RUSTC':paths['rustc'],'RUSTDOC':paths['rustdoc'],
        'CC':wrappers['CC'],'AR':wrappers['AR'],'CC_ENABLE_DEBUG_OUTPUT':'1',
        'PCW_NATIVE_C_RECORD_CONFIG':native,
        'PCW_NATIVE_C_RECORD_CONFIG_SHA256':review['nativeConfigSHA256'],
        **sdk,
    }
    encoded=json.dumps(environment,sort_keys=True,separators=(',',':')).encode()
    receipt={
        'schemaVersion':1,'kind':'closed-cargo-environment',
        'reviewSHA256':sha(json.dumps(review,sort_keys=True,separators=(',',':')).encode()),
        'producerSourceSHA256':sha(pathlib.Path(__file__).read_bytes()),
        'parentQualification':{'exactKeys':sorted(FORBIDDEN),'flagFamilies':list(FLAG_FAMILIES),'injectionPrefixes':list(INJECTION_PREFIXES),'targetRule':'CARGO_TARGET_*_(RUSTFLAGS|LINKER|RUNNER)','observed':observed,'refusedKeys':[]},
        'inheritedEnvironment':False,'cargoProgram':paths['cargo'],
        'cargoChildEnvironment':environment,'cargoChildEnvironmentSHA256':sha(encoded),
        'evidenceLevel':'prepared-environment-only-not-actual-Cargo-or-native-build',
    }
    return environment,receipt
