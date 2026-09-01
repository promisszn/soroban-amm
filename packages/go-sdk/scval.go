package gosdk

import (
	"encoding/base64"
	"errors"
	"fmt"
	"math/big"
)

// ErrI128OutOfRange is returned when a value cannot be represented as an i128.
var ErrI128OutOfRange = errors.New("value out of i128 range")

// ErrU128OutOfRange is returned when a value cannot be represented as a u128.
var ErrU128OutOfRange = errors.New("value out of u128 range")

var (
	// MaxI128 is 2^127 - 1, the largest value representable as an i128.
	MaxI128 = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), 127), big.NewInt(1))
	// MinI128 is -2^127, the smallest value representable as an i128.
	MinI128 = new(big.Int).Neg(new(big.Int).Lsh(big.NewInt(1), 127))
	// MaxU128 is 2^128 - 1, the largest value representable as a u128.
	MaxU128 = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), 128), big.NewInt(1))

	two128 = new(big.Int).Lsh(big.NewInt(1), 128)
)

// Parts128 is the two-limb representation Soroban uses on the wire for 128-bit
// integers: an ordered pair of a high and a low 64-bit limb.
type Parts128 struct {
	Hi uint64
	Lo uint64
}

// EncodeI128 splits a signed 128-bit integer into its two's-complement high and
// low limbs. Values outside the i128 range are rejected rather than truncated.
func EncodeI128(v *big.Int) (Parts128, error) {
	if v == nil {
		return Parts128{}, fmt.Errorf("%w: nil value", ErrI128OutOfRange)
	}
	if v.Cmp(MaxI128) > 0 || v.Cmp(MinI128) < 0 {
		return Parts128{}, fmt.Errorf("%w: %s", ErrI128OutOfRange, v.String())
	}

	u := new(big.Int).Set(v)
	if u.Sign() < 0 {
		u.Add(u, two128)
	}

	lo := new(big.Int).And(u, new(big.Int).SetUint64(^uint64(0)))
	hi := new(big.Int).Rsh(u, 64)

	return Parts128{Hi: hi.Uint64(), Lo: lo.Uint64()}, nil
}

// DecodeI128 reassembles a signed 128-bit integer from its two's-complement
// high and low limbs.
func DecodeI128(p Parts128) *big.Int {
	u := new(big.Int).SetUint64(p.Hi)
	u.Lsh(u, 64)
	u.Or(u, new(big.Int).SetUint64(p.Lo))

	if u.Cmp(MaxI128) > 0 {
		u.Sub(u, two128)
	}
	return u
}

// EncodeU128 splits an unsigned 128-bit integer into its high and low limbs.
// Negative values and values above 2^128-1 are rejected.
func EncodeU128(v *big.Int) (Parts128, error) {
	if v == nil {
		return Parts128{}, fmt.Errorf("%w: nil value", ErrU128OutOfRange)
	}
	if v.Sign() < 0 || v.Cmp(MaxU128) > 0 {
		return Parts128{}, fmt.Errorf("%w: %s", ErrU128OutOfRange, v.String())
	}

	lo := new(big.Int).And(v, new(big.Int).SetUint64(^uint64(0)))
	hi := new(big.Int).Rsh(v, 64)

	return Parts128{Hi: hi.Uint64(), Lo: lo.Uint64()}, nil
}

// DecodeU128 reassembles an unsigned 128-bit integer from its limbs.
func DecodeU128(p Parts128) *big.Int {
	u := new(big.Int).SetUint64(p.Hi)
	u.Lsh(u, 64)
	return u.Or(u, new(big.Int).SetUint64(p.Lo))
}

// EncodeI128Bytes returns the 16-byte big-endian two's-complement encoding of a
// signed 128-bit integer.
func EncodeI128Bytes(v *big.Int) ([]byte, error) {
	p, err := EncodeI128(v)
	if err != nil {
		return nil, err
	}
	out := make([]byte, 16)
	putUint64BE(out[0:8], p.Hi)
	putUint64BE(out[8:16], p.Lo)
	return out, nil
}

// DecodeI128Bytes parses a 16-byte big-endian two's-complement i128.
func DecodeI128Bytes(b []byte) (*big.Int, error) {
	if len(b) != 16 {
		return nil, fmt.Errorf("i128: expected 16 bytes, got %d", len(b))
	}
	return DecodeI128(Parts128{Hi: uint64BE(b[0:8]), Lo: uint64BE(b[8:16])}), nil
}

// EncodeI128Base64 returns the base64 form of the 16-byte i128 encoding, which
// is how the JSON-RPC layer carries the value.
func EncodeI128Base64(v *big.Int) (string, error) {
	b, err := EncodeI128Bytes(v)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(b), nil
}

// DecodeI128Base64 parses a base64-encoded 16-byte i128.
func DecodeI128Base64(s string) (*big.Int, error) {
	b, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return nil, fmt.Errorf("i128: invalid base64: %w", err)
	}
	return DecodeI128Bytes(b)
}

func putUint64BE(dst []byte, v uint64) {
	for i := 0; i < 8; i++ {
		dst[i] = byte(v >> (56 - 8*i))
	}
}

func uint64BE(src []byte) uint64 {
	var v uint64
	for i := 0; i < 8; i++ {
		v = v<<8 | uint64(src[i])
	}
	return v
}
