#!/usr/bin/env python3
"""Verify actual crewd-schema output against the frozen public33/235 contract.

This is a reader/qualifier, never a JSON Schema generator or publisher. Only the
original crewd CLI -> crewd_proto::api_schema may produce the input artifact.
"""
import argparse
import hashlib
import json
import pathlib
import re

BASE_COUNT = 235
BASE_CANONICAL_SHA256 = 'c8c0de2f17870c7a931105a4d0dd9bbd3fb0eb36a26079d800282a34213f93d5'
READ = 'adminCase.secretaryBrief'
WRITE = 'adminCase.secretaryRequest'
NEW_DEFS = {'AdminSecretaryGet', 'AdminSecretaryView', 'AdminSecretaryState',
            'AdminSecretarySnapshot', 'AdminSecretaryAuthor', 'AdminSecretaryPublication',
            'AdminSecretaryReference', 'AdminSecretaryRequest', 'AdminSecretaryRequestResult'}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def load(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            assert key not in result, 'duplicate JSON key: ' + key
            result[key] = value
        return result
    return json.loads(path.read_bytes(), object_pairs_hook=unique)


def closed_variant(definition, key, name):
    matches = [v for v in definition['oneOf'] if v.get('properties', {}).get(key, {}).get('const') == name]
    assert len(matches) == 1, 'missing/duplicate closed variant: ' + name
    assert matches[0]['additionalProperties'] is False
    return matches[0]


def verify(baseline, actual, source):
    assert len(baseline['methods']) == BASE_COUNT
    assert hashlib.sha256(canonical(baseline)).hexdigest() == BASE_CANONICAL_SHA256, 'not the frozen public33 baseline'
    assert set(actual['methods']) - set(baseline['methods']) == {READ}
    assert set(baseline['methods']) <= set(actual['methods'])
    assert len(actual['methods']) == BASE_COUNT + 1
    for name, method in baseline['methods'].items():
        assert actual['methods'][name] == method, 'existing method changed: ' + name
    assert actual['methods'][READ] == {'params': {'$ref': '#/$defs/AdminSecretaryGet'}, 'result': {'$ref': '#/$defs/AdminSecretaryView'}}
    assert WRITE not in actual['methods'], 'the writing-source request is command-only'
    assert set(actual) == set(baseline)
    for key in set(baseline) - {'methods', '$defs'}:
        assert actual[key] == baseline[key], 'existing top-level contract changed: ' + key
    before, after = baseline['$defs'], actual['$defs']
    assert set(before) <= set(after)
    assert set(after) - set(before) == NEW_DEFS, 'missing or unreviewed/private DTO appeared'
    for name, definition in before.items():
        if name not in {'PublicCommand', 'AdminCaseProvenance'}:
            assert after[name] == definition, 'existing DTO changed: ' + name
    command = closed_variant(after['PublicCommand'], 'operation', WRITE)
    assert command == {'additionalProperties': False, 'properties': {'operation': {'const': WRITE, 'type': 'string'}, 'params': {'$ref': '#/$defs/AdminSecretaryRequest'}}, 'required': ['operation', 'params'], 'type': 'object'}
    for name, key, extra in [('PublicCommand', 'operation', WRITE), ('AdminCaseProvenance', 'kind', 'secretary_brief')]:
        definition = dict(after[name])
        variants = [v for v in definition['oneOf'] if v.get('properties', {}).get(key, {}).get('const') != extra]
        assert len(variants) + 1 == len(definition['oneOf'])
        definition['oneOf'] = variants
        assert definition == before[name], 'original variants changed: ' + name
    result = after['AdminSecretaryRequestResult']
    assert result['additionalProperties'] is False
    assert set(result['properties']) == {'crewId', 'sourceId', 'caseId', 'sourceRevision'}
    assert set(result['required']) == set(result['properties'])
    snapshot = after['AdminSecretarySnapshot']
    assert snapshot['additionalProperties'] is False and 'patrolRevision' in snapshot['properties']
    version_source = (source / 'crewd/crates/store/src/schema.rs').read_text()
    assert re.findall(r'^pub const SCHEMA_VERSION: i64 = ([0-9]+);$', version_source, re.MULTILINE) == ['34']
    return {'methodCount': len(actual['methods']), 'storeVersion': 34,
            'canonicalSchemaSHA256': hashlib.sha256(canonical(actual)).hexdigest(),
            'baselineCanonicalSHA256': BASE_CANONICAL_SHA256,
            'newRead': READ, 'commandOnly': WRITE,
            'existingMethodsAndDTOsPreserved': True,
            'evidence': 'actual CLI output must be supplied by Root; verifier does not generate or execute it'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=pathlib.Path, required=True)
    parser.add_argument('--generated', type=pathlib.Path, required=True)
    parser.add_argument('--source', type=pathlib.Path, required=True)
    args = parser.parse_args()
    result = verify(load(args.baseline), load(args.generated), args.source)
    result['generatedFileSHA256'] = hashlib.sha256(args.generated.read_bytes()).hexdigest()
    print(json.dumps(result, sort_keys=True, indent=2))


if __name__ == '__main__':
    main()
