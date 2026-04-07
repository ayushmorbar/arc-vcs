# SemVer BNF Grammar (Full)

```
<valid semver> ::= <version core>
                 | <version core> "-" <pre-release>
                 | <version core> "+" <build>
                 | <version core> "-" <pre-release> "+" <build>

<version core> ::= <major> "." <minor> "." <patch>
<major> ::= <numeric identifier>
<minor> ::= <numeric identifier>
<patch> ::= <numeric identifier>

<pre-release> ::= <dot-separated pre-release identifiers>
<dot-separated pre-release identifiers> ::=
    <pre-release identifier>
  | <pre-release identifier> "." <dot-separated pre-release identifiers>

<build> ::= <dot-separated build identifiers>
<dot-separated build identifiers> ::=
    <build identifier>
  | <build identifier> "." <dot-separated build identifiers>

<pre-release identifier> ::= <alphanumeric identifier> | <numeric identifier>
<build identifier>        ::= <alphanumeric identifier> | <digits>

<alphanumeric identifier> ::=
    <non-digit>
  | <non-digit> <identifier characters>
  | <identifier characters> <non-digit>
  | <identifier characters> <non-digit> <identifier characters>

<numeric identifier> ::= "0" | <positive digit> | <positive digit> <digits>

<identifier characters> ::=
    <identifier character>
  | <identifier character> <identifier characters>

<identifier character> ::= <digit> | <non-digit>
<non-digit>             ::= <letter> | "-"
<digits>                ::= <digit> | <digit> <digits>
<digit>                 ::= "0" | <positive digit>
<positive digit>        ::= "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"
<letter>                ::= "A"|"B"|...|"Z"|"a"|"b"|...|"z"
```

Source: https://semver.org/spec/v2.0.0.html