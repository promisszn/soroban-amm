package gosdk

import (
	"encoding/base64"
	"math/big"
	"strings"
	"testing"
)

func validSpec() InvokeSpec {
	return InvokeSpec{
		SourceAccount:     zeroAccount,
		ContractID:        zeroContract,
		Method:            "get_amount_out",
		Args:              []ScValue{Addr(zeroContract), I128(big.NewInt(1_000))},
		NetworkPassphrase: NetworkTestnet,
	}
}

func TestBuildInvokeEnvelopeStructure(t *testing.T) {
	encoded, err := BuildInvokeEnvelope(validSpec())
	if err != nil {
		t.Fatalf("BuildInvokeEnvelope: %v", err)
	}
	raw, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatalf("envelope is not valid base64: %v", err)
	}
	if len(raw)%4 != 0 {
		t.Fatalf("envelope is %d bytes, which is not XDR-aligned", len(raw))
	}

	r := &xdrReader{buf: raw}

	if got := readInt32OrFail(t, r); got != envelopeTypeTx {
		t.Fatalf("envelope type = %d, want %d", got, envelopeTypeTx)
	}
	if got := readInt32OrFail(t, r); got != muxedAccountEd25519 {
		t.Fatalf("muxed account type = %d, want %d", got, muxedAccountEd25519)
	}
	source, err := r.readRaw(32)
	if err != nil {
		t.Fatalf("reading source account: %v", err)
	}
	wantSource, _, err := DecodeStrkey(zeroAccount)
	if err != nil {
		t.Fatalf("decoding the expected source: %v", err)
	}
	if string(source) != string(wantSource) {
		t.Fatal("envelope carries the wrong source account")
	}

	if fee := readUint32OrFail(t, r); fee != BaseFee {
		t.Fatalf("fee = %d, want %d", fee, BaseFee)
	}
	if seq, err := r.readUint64(); err != nil || seq != 0 {
		t.Fatalf("sequence = %d (err %v), want 0", seq, err)
	}
	if cond := readInt32OrFail(t, r); cond != txPreconditionsNone {
		t.Fatalf("preconditions = %d, want none", cond)
	}
	if memo := readInt32OrFail(t, r); memo != 0 {
		t.Fatalf("memo = %d, want MEMO_NONE", memo)
	}
	if ops := readUint32OrFail(t, r); ops != 1 {
		t.Fatalf("operation count = %d, want 1", ops)
	}
	if present := readUint32OrFail(t, r); present != 0 {
		t.Fatal("the operation should not carry its own source account")
	}
	if op := readInt32OrFail(t, r); op != opTypeInvokeHostFn {
		t.Fatalf("operation type = %d, want %d", op, opTypeInvokeHostFn)
	}
	if fn := readInt32OrFail(t, r); fn != hostFnTypeInvoke {
		t.Fatalf("host function type = %d, want %d", fn, hostFnTypeInvoke)
	}
	if kind := readInt32OrFail(t, r); kind != scAddressCntrct {
		t.Fatalf("contract address kind = %d, want %d", kind, scAddressCntrct)
	}
	if _, err := r.readRaw(32); err != nil {
		t.Fatalf("reading contract id: %v", err)
	}

	name, err := r.readBytes()
	if err != nil {
		t.Fatalf("reading function name: %v", err)
	}
	if string(name) != "get_amount_out" {
		t.Fatalf("function = %q, want get_amount_out", name)
	}

	if argc := readUint32OrFail(t, r); argc != 2 {
		t.Fatalf("argument count = %d, want 2", argc)
	}

	first, err := readScVal(r)
	if err != nil {
		t.Fatalf("reading argument 0: %v", err)
	}
	if addr, err := first.Address(); err != nil || addr != zeroContract {
		t.Fatalf("argument 0 = %v (err %v), want the token address", addr, err)
	}

	second, err := readScVal(r)
	if err != nil {
		t.Fatalf("reading argument 1: %v", err)
	}
	amount, err := second.BigInt()
	if err != nil || amount.Int64() != 1_000 {
		t.Fatalf("argument 1 = %v (err %v), want 1000", amount, err)
	}
}

func TestBuildInvokeEnvelopeWritesTimeBounds(t *testing.T) {
	spec := validSpec()
	spec.TimeoutSeconds = 1_900_000_000
	spec.Sequence = 42
	spec.Fee = 500

	encoded, err := BuildInvokeEnvelope(spec)
	if err != nil {
		t.Fatalf("BuildInvokeEnvelope: %v", err)
	}
	raw, _ := base64.StdEncoding.DecodeString(encoded)
	r := &xdrReader{buf: raw}

	readInt32OrFail(t, r) // envelope type
	readInt32OrFail(t, r) // muxed account type
	if _, err := r.readRaw(32); err != nil {
		t.Fatalf("reading source: %v", err)
	}
	if fee := readUint32OrFail(t, r); fee != 500 {
		t.Fatalf("fee = %d, want 500", fee)
	}
	seq, err := r.readUint64()
	if err != nil || seq != 42 {
		t.Fatalf("sequence = %d (err %v), want 42", seq, err)
	}
	if cond := readInt32OrFail(t, r); cond != txPreconditionsTime {
		t.Fatalf("preconditions = %d, want time bounds", cond)
	}
	if lower, err := r.readUint64(); err != nil || lower != 0 {
		t.Fatalf("min time = %d (err %v), want 0", lower, err)
	}
	upper, err := r.readUint64()
	if err != nil || upper != 1_900_000_000 {
		t.Fatalf("max time = %d (err %v), want 1900000000", upper, err)
	}
}

func TestBuildInvokeEnvelopeRejectsBadSpecs(t *testing.T) {
	cases := map[string]func(*InvokeSpec){
		"empty source":         func(s *InvokeSpec) { s.SourceAccount = "" },
		"contract as source":   func(s *InvokeSpec) { s.SourceAccount = zeroContract },
		"empty contract":       func(s *InvokeSpec) { s.ContractID = "" },
		"account as contract":  func(s *InvokeSpec) { s.ContractID = zeroAccount },
		"empty method":         func(s *InvokeSpec) { s.Method = "" },
		"over-long method":     func(s *InvokeSpec) { s.Method = strings.Repeat("a", 33) },
		"unencodable argument": func(s *InvokeSpec) { s.Args = []ScValue{Addr("bogus")} },
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			spec := validSpec()
			mutate(&spec)
			if _, err := BuildInvokeEnvelope(spec); err == nil {
				t.Fatal("expected an error")
			}
		})
	}
}

func TestBuildInvokeEnvelopeAcceptsNoArguments(t *testing.T) {
	spec := validSpec()
	spec.Method = "get_info"
	spec.Args = nil

	if _, err := BuildInvokeEnvelope(spec); err != nil {
		t.Fatalf("a zero-argument call should encode: %v", err)
	}
}

func readInt32OrFail(t *testing.T, r *xdrReader) int32 {
	t.Helper()
	v, err := r.readInt32()
	if err != nil {
		t.Fatalf("reading int32: %v", err)
	}
	return v
}

func readUint32OrFail(t *testing.T, r *xdrReader) uint32 {
	t.Helper()
	v, err := r.readUint32()
	if err != nil {
		t.Fatalf("reading uint32: %v", err)
	}
	return v
}
