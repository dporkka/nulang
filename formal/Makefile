COQC = coqc
COQDEP = coqdep

VFILES = Syntax.v Typing.v Semantics.v
VOFILES = $(VFILES:.v=.vo)

all: $(VOFILES)

%.vo: %.v
	$(COQC) $<

clean:
	rm -f *.vo *.glob .*.aux
