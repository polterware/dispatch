package setup

import (
	"errors"
	"fmt"
)

// span is a value's byte range in the source, end exclusive.
type span struct{ start, end int }

// jsoncDoc is what editing the plugins list needs of a JSON or JSONC object:
// where it opens, how many members it has, and the plugins list's range and
// elements.
type jsoncDoc struct {
	open         int
	members      int
	pluginsKey   bool
	pluginsArray *span
	plugins      []span
}

type jsoncScanner struct {
	src []byte
	pos int
}

// parseJSONC reads the top-level object of src, which may hold comments and
// trailing commas.
func parseJSONC(src []byte) (jsoncDoc, error) {
	s := &jsoncScanner{src: src}
	var doc jsoncDoc
	s.space()
	if s.peek() != '{' {
		return doc, errors.New("the config is not a JSON object")
	}
	doc.open = s.pos
	s.pos++
	for {
		s.space()
		if s.peek() == '}' {
			return doc, nil
		}
		key, err := s.str()
		if err != nil {
			return doc, err
		}
		s.space()
		if s.peek() != ':' {
			return doc, s.errorf("expected ':'")
		}
		s.pos++
		s.space()
		doc.members++
		if key == `"plugins"` {
			doc.pluginsKey = true
			if s.peek() == '[' {
				arr, els, err := s.array()
				if err != nil {
					return doc, err
				}
				doc.pluginsArray, doc.plugins = &arr, els
			} else if _, err := s.value(); err != nil {
				return doc, err
			}
		} else if _, err := s.value(); err != nil {
			return doc, err
		}
		s.space()
		switch s.peek() {
		case ',':
			s.pos++
		case '}':
		default:
			return doc, s.errorf("expected ',' or '}'")
		}
	}
}

func (s *jsoncScanner) errorf(format string, args ...any) error {
	return fmt.Errorf("invalid JSON at byte %d: %s", s.pos, fmt.Sprintf(format, args...))
}

func (s *jsoncScanner) peek() byte {
	if s.pos >= len(s.src) {
		return 0
	}
	return s.src[s.pos]
}

// space skips whitespace and comments.
func (s *jsoncScanner) space() {
	for s.pos < len(s.src) {
		switch c := s.src[s.pos]; {
		case c == ' ' || c == '\t' || c == '\n' || c == '\r':
			s.pos++
		case c == '/' && s.pos+1 < len(s.src) && s.src[s.pos+1] == '/':
			for s.pos < len(s.src) && s.src[s.pos] != '\n' {
				s.pos++
			}
		case c == '/' && s.pos+1 < len(s.src) && s.src[s.pos+1] == '*':
			s.pos += 2
			for s.pos+1 < len(s.src) && (s.src[s.pos] != '*' || s.src[s.pos+1] != '/') {
				s.pos++
			}
			s.pos += 2
		default:
			return
		}
	}
}

// str reads a string and returns it with its quotes.
func (s *jsoncScanner) str() (string, error) {
	if s.peek() != '"' {
		return "", s.errorf("expected a string")
	}
	start := s.pos
	s.pos++
	for s.pos < len(s.src) {
		switch s.src[s.pos] {
		case '\\':
			s.pos += 2
		case '"':
			s.pos++
			return string(s.src[start:s.pos]), nil
		default:
			s.pos++
		}
	}
	return "", s.errorf("unterminated string")
}

// value skips one value and returns its range.
func (s *jsoncScanner) value() (span, error) {
	start := s.pos
	switch c := s.peek(); {
	case c == '"':
		_, err := s.str()
		return span{start, s.pos}, err
	case c == '[':
		arr, _, err := s.array()
		return arr, err
	case c == '{':
		s.pos++
		for {
			s.space()
			if s.peek() == '}' {
				s.pos++
				return span{start, s.pos}, nil
			}
			if _, err := s.str(); err != nil {
				return span{}, err
			}
			s.space()
			if s.peek() != ':' {
				return span{}, s.errorf("expected ':'")
			}
			s.pos++
			s.space()
			if _, err := s.value(); err != nil {
				return span{}, err
			}
			s.space()
			switch s.peek() {
			case ',':
				s.pos++
			case '}':
			default:
				return span{}, s.errorf("expected ',' or '}'")
			}
		}
	case c == 0:
		return span{}, s.errorf("unexpected end")
	default:
		for s.pos < len(s.src) {
			c := s.src[s.pos]
			if c == ',' || c == '}' || c == ']' || c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '/' {
				break
			}
			s.pos++
		}
		if s.pos == start {
			return span{}, s.errorf("expected a value")
		}
		return span{start, s.pos}, nil
	}
}

// array reads an array and returns its range and its elements' ranges.
func (s *jsoncScanner) array() (span, []span, error) {
	start := s.pos
	s.pos++
	var els []span
	for {
		s.space()
		if s.peek() == ']' {
			s.pos++
			return span{start, s.pos}, els, nil
		}
		el, err := s.value()
		if err != nil {
			return span{}, nil, err
		}
		els = append(els, el)
		s.space()
		switch s.peek() {
		case ',':
			s.pos++
		case ']':
		default:
			return span{}, nil, s.errorf("expected ',' or ']'")
		}
	}
}

// stripJSONC drops comments and trailing commas so encoding/json reads a
// value.
func stripJSONC(src []byte) []byte {
	out := make([]byte, 0, len(src))
	s := &jsoncScanner{src: src}
	for s.pos < len(src) {
		c := src[s.pos]
		switch {
		case c == '"':
			start := s.pos
			if _, err := s.str(); err != nil {
				return append(out, src[start:]...)
			}
			out = append(out, src[start:s.pos]...)
		case c == '/' && s.pos+1 < len(src) && (src[s.pos+1] == '/' || src[s.pos+1] == '*'):
			s.space()
		case c == ',':
			s.pos++
			s.space()
			if next := s.peek(); next != '}' && next != ']' {
				out = append(out, ',')
			}
		default:
			out = append(out, c)
			s.pos++
		}
	}
	return out
}
