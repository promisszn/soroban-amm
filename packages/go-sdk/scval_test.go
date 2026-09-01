package gosdk

import (
	"errors"
	"math/big"
	"testing"
)

func mustBig(t *testing.T, s string) *big.Int {
	t.Helper()
	v, ok := new(big.Int).SetString(s, 10)
	if !ok {
		t.Fatalf("bad literal %q", s)
	}
	return v
}

func TestI128RoundTrip(t *testing.T) {
	cases := []struct {
		name  string
		value string
	}{
		{"zero", "0"},
		{"one", "1"},
		{"negative one", "-1"},
		{"max i64", "9223372036854775807"},
		{"min i64", "-9223372036854775808"},
		{"max i64 plus one", "9223372036854775808"},
		{"max i128", "170141183460469231731687303715884105727"},
		{"min i128", "-170141183460469231731687303715884105728"},
		{"typical amount", "1000000000000"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			want := mustBig(t, tc.value)

			parts, err := EncodeI128(want)
			if err != nil {
				t.Fatalf("EncodeI128(%s): %v", tc.value, err)
			}
			if got := DecodeI128(parts); got.Cmp(want) != 0 {
				t.Fatalf("limb round-trip: got %s want %s", got, want)
			}

			b, err := EncodeI128Bytes(want)
			if err != nil {
				t.Fatalf("EncodeI128Bytes(%s): %v", tc.value, err)
			}
			if len(b) != 16 {
				t.Fatalf("expected 16 bytes, got %d", len(b))
			}
			gotBytes, err := DecodeI128Bytes(b)
			if err != nil {
				t.Fatalf("DecodeI128Bytes: %v", err)
			}
			if gotBytes.Cmp(want) != 0 {
				t.Fatalf("byte round-trip: got %s want %s", gotBytes, want)
			}

			s, err := EncodeI128Base64(want)
			if err != nil {
				t.Fatalf("EncodeI128Base64: %v", err)
			}
			gotB64, err := DecodeI128Base64(s)
			if err != nil {
				t.Fatalf("DecodeI128Base64: %v", err)
			}
			if gotB64.Cmp(want) != 0 {
				t.Fatalf("base64 round-trip: got %s want %s", gotB64, want)
			}
		})
	}
}

func TestI128RejectsOutOfRange(t *testing.T) {
	cases := []struct {
		name  string
		value *big.Int
	}{
		{"max i128 plus one", new(big.Int).Add(MaxI128, big.NewInt(1))},
		{"min i128 minus one", new(big.Int).Sub(MinI128, big.NewInt(1))},
		{"far above range", new(big.Int).Mul(MaxI128, big.NewInt(4))},
		{"far below range", new(big.Int).Mul(MinI128, big.NewInt(4))},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := EncodeI128(tc.value); !errors.Is(err, ErrI128OutOfRange) {
				t.Fatalf("expected ErrI128OutOfRange, got %v", err)
			}
			if _, err := EncodeI128Bytes(tc.value); !errors.Is(err, ErrI128OutOfRange) {
				t.Fatalf("bytes: expected ErrI128OutOfRange, got %v", err)
			}
		})
	}
}

func TestI128RejectsNil(t *testing.T) {
	if _, err := EncodeI128(nil); !errors.Is(err, ErrI128OutOfRange) {
		t.Fatalf("expected ErrI128OutOfRange for nil, got %v", err)
	}
}

func TestI128NegativeLimbEncoding(t *testing.T) {
	parts, err := EncodeI128(big.NewInt(-1))
	if err != nil {
		t.Fatalf("EncodeI128(-1): %v", err)
	}
	if parts.Hi != ^uint64(0) || parts.Lo != ^uint64(0) {
		t.Fatalf("-1 should be all ones, got hi=%d lo=%d", parts.Hi, parts.Lo)
	}
}

func TestU128RoundTrip(t *testing.T) {
	cases := []string{
		"0",
		"1",
		"18446744073709551615",
		"18446744073709551616",
		"340282366920938463463374607431768211455",
	}

	for _, c := range cases {
		want := mustBig(t, c)
		parts, err := EncodeU128(want)
		if err != nil {
			t.Fatalf("EncodeU128(%s): %v", c, err)
		}
		if got := DecodeU128(parts); got.Cmp(want) != 0 {
			t.Fatalf("u128 round-trip: got %s want %s", got, want)
		}
	}
}

func TestU128RejectsOutOfRange(t *testing.T) {
	if _, err := EncodeU128(big.NewInt(-1)); !errors.Is(err, ErrU128OutOfRange) {
		t.Fatalf("expected ErrU128OutOfRange for -1, got %v", err)
	}
	over := new(big.Int).Add(MaxU128, big.NewInt(1))
	if _, err := EncodeU128(over); !errors.Is(err, ErrU128OutOfRange) {
		t.Fatalf("expected ErrU128OutOfRange for 2^128, got %v", err)
	}
	if _, err := EncodeU128(nil); !errors.Is(err, ErrU128OutOfRange) {
		t.Fatalf("expected ErrU128OutOfRange for nil, got %v", err)
	}
}

func TestDecodeI128BytesRejectsWrongLength(t *testing.T) {
	for _, n := range []int{0, 1, 15, 17, 32} {
		if _, err := DecodeI128Bytes(make([]byte, n)); err == nil {
			t.Fatalf("expected error for %d-byte input", n)
		}
	}
}

func TestDecodeI128Base64RejectsGarbage(t *testing.T) {
	if _, err := DecodeI128Base64("!!!not base64!!!"); err == nil {
		t.Fatal("expected error for invalid base64")
	}
}
