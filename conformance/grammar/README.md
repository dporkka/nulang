# Grammar Conformance Corpus

This directory holds the authoritative test cases for Nulang's parser.
The reference grammar is `spec/grammar.ebnf`. The cases here (positive: must parse, negative: must reject) are executed by the `test_grammar_conformance` harness in `src/integration_tests.rs`.

Any syntax change via RFC must be accompanied by an update to the EBNF and new cases here.
