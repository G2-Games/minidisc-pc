use crate::netmd::utils;
use thiserror::Error;

/// %b, w, d, q - explained above (can have endiannes overriden by '>' and '<' operators, f. ex. %>d %<q)
/// %s - Uint8Array preceded by 2 bytes of length
/// %x - Uint8Array preceded by 2 bytes of length
/// %z - Uint8Array preceded by 1 byte of length
/// %* - raw Uint8Array
/// %B - BCD-encoded 1-byte number
/// %W - BCD-encoded 2-byte number
static FORMAT_TYPE_LEN_DICT: phf::Map<char, i32> = phf::phf_map! {
    'b' => 1, // byte
    'w' => 2, // word
    'd' => 4, // doubleword
    'q' => 8, // quadword
};

const DEBUG: bool = false;

#[derive(Clone, Debug)]
pub enum QueryValue {
    Number(i64),
    Array(Vec<u8>),
}

#[derive(Error, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ValueError {
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
}

impl QueryValue {
    pub fn _from_array<const S: usize>(value: [u8; S]) -> Self {
        Self::Array(value.to_vec())
    }

    pub fn _to_array<const S: usize>(&self) -> Result<[u8; S], ValueError> {
        let mut array = [0u8; S];
        match self {
            QueryValue::Array(a) => {
                for (i, byte) in a.iter().take(S).enumerate() {
                    array[i] = *byte
                }
                Ok(array)
            }
            _ => Err(ValueError::TypeMismatch {
                expected: String::from("Vec<u8>"),
                actual: format!("{:?}", self),
            }),
        }
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, ValueError> {
        match self {
            QueryValue::Array(a) => Ok(a.to_vec()),
            _ => Err(ValueError::TypeMismatch {
                expected: String::from("Vec<u8>"),
                actual: format!("{:?}", self),
            }),
        }
    }

    pub fn to_i64(&self) -> Result<i64, ValueError> {
        match self {
            QueryValue::Number(a) => Ok(*a),
            _ => Err(ValueError::TypeMismatch {
                expected: String::from("i64"),
                actual: format!("{:?}", self),
            }),
        }
    }
}

impl TryInto<i64> for QueryValue {
    type Error = ValueError;

    fn try_into(self) -> Result<i64, Self::Error> {
        match self {
            QueryValue::Number(a) => Ok(a),
            _ => Err(ValueError::TypeMismatch {
                expected: String::from("i64"),
                actual: format!("{:?}", self),
            }),
        }
    }
}

#[derive(Error, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum QueryError {
    #[error("unrecognized format character: `{0}`")]
    UnrecognizedChar(char),

    #[error("Format and input mismatch at {index}: expected {expected:#04x}, got {actual:#04x} (format {format_string})")]
    InputMismatch {
        index: usize,
        expected: u8,
        actual: u8,
        format_string: String,
    },
}

/// Formats a query using a standard input to send to the player
pub fn format_query(format: String, args: Vec<QueryValue>) -> Result<Vec<u8>, QueryError> {
    if DEBUG {
        println!("SENT>>> F: {}", format);
    }

    let mut result: Vec<u8> = Vec::new();
    let mut half: Option<char> = None;
    let mut arg_stack = args.into_iter();
    let mut endianness_override: Option<char> = None;

    let mut escaped = false;
    for character in format.chars() {
        if escaped {
            if endianness_override.is_none() && ['<', '>'].contains(&character) {
                endianness_override = Some(character);
                continue;
            }
            escaped = false;

            match character {
                character if FORMAT_TYPE_LEN_DICT.contains_key(&character) => {
                    let value = arg_stack.next().unwrap().to_i64().unwrap();
                    match character {
                        'b' => result.push(value as u8),
                        'w' => {
                            let mut value_bytes = (value as i16).to_be_bytes().to_vec();
                            result.append(&mut value_bytes)
                        }
                        'd' => {
                            let mut value_bytes = (value as i32).to_be_bytes().to_vec();
                            result.append(&mut value_bytes)
                        }
                        'q' => {
                            let mut value_bytes = value.to_be_bytes().to_vec();
                            result.append(&mut value_bytes)
                        }
                        _ => (),
                    };
                    endianness_override = None;
                }
                character if character == 'x' || character == 's' || character == 'z' => {
                    let mut array_value = arg_stack.next().unwrap().to_vec().unwrap();

                    let mut array_length = array_value.len();

                    if character == 's' {
                        array_length += 1;
                    }

                    if character != 'z' {
                        result.push(((array_length >> 8) & 0xFF) as u8)
                    }
                    result.push((array_length & 0xFF) as u8);
                    result.append(&mut array_value);
                    if character == 's' {
                        result.push(0);
                    }
                }
                '*' => {
                    let mut array_value = arg_stack.next().unwrap().to_vec().unwrap();
                    result.append(&mut array_value);
                }
                character if character == 'B' || character == 'W' => {
                    let value = arg_stack.next().unwrap().to_i64().unwrap();
                    let converted = utils::int_to_bcd(value as i32);
                    if character == 'W' {
                        result.push(((converted >> 8) & 0xFF) as u8);
                    }
                    result.push((converted & 0xFF) as u8);
                }
                _ => return Err(QueryError::UnrecognizedChar(character)),
            }
            continue;
        }
        if character == '%' {
            escaped = true;
            continue;
        }
        if character == ' ' {
            continue;
        }
        if half.is_none() {
            half = Some(character);
        } else {
            result.push(
                u8::from_str_radix(&String::from_iter([half.unwrap(), character]), 16).unwrap(),
            );
            half = None;
        }
    }

    Ok(result)
}

/// Scans a result using a standard input to recieve from the player
pub fn scan_query(query_result: Vec<u8>, format: String) -> Result<Vec<QueryValue>, QueryError> {
    let mut result: Vec<QueryValue> = Vec::new();

    let initial_length = query_result.len();
    let mut input_stack = query_result.into_iter();
    let mut half: Option<char> = None;
    let mut endianness_override: Option<char> = None;
    let mut escaped = false;

    // Remove an unknown byte at the beginning
    // TODO: Find out what this is
    input_stack.next();

    for character in format.chars() {
        if escaped {
            if endianness_override.is_none() && ['<', '>'].contains(&character) {
                endianness_override = Some(character);
                continue;
            }
            escaped = false;

            if character == '?' {
                input_stack.next();
                continue;
            }

            match character {
                character if FORMAT_TYPE_LEN_DICT.contains_key(&character) => {
                    match character {
                        'b' => {
                            let new_value =
                                u8::from_be_bytes(utils::get_bytes(&mut input_stack).unwrap());
                            result.push(QueryValue::Number(new_value as i64));
                        }
                        'w' => {
                            let new_value =
                                i16::from_be_bytes(utils::get_bytes(&mut input_stack).unwrap());
                            result.push(QueryValue::Number(new_value as i64));
                        }
                        'd' => {
                            let new_value =
                                i32::from_be_bytes(utils::get_bytes(&mut input_stack).unwrap());
                            result.push(QueryValue::Number(new_value as i64));
                        }
                        'q' => {
                            let new_value =
                                i64::from_be_bytes(utils::get_bytes(&mut input_stack).unwrap());
                            result.push(QueryValue::Number(new_value));
                        }
                        _ => unreachable!(),
                    };
                    endianness_override = None;
                }
                character if character == 'x' || character == 's' || character == 'z' => {
                    let length = match character {
                        'z' => input_stack.next().unwrap() as u16,
                        _ => u16::from_be_bytes(utils::get_bytes(&mut input_stack).unwrap()),
                    };
                    let mut result_buffer: Vec<u8> = Vec::new();
                    for _ in 0..length {
                        result_buffer.push(input_stack.next().unwrap());
                    }
                    result.push(QueryValue::Array(result_buffer))
                }
                character if character == '*' || character == '#' => {
                    let mut result_buffer: Vec<u8> = Vec::new();
                    let temp_stack = input_stack.clone();
                    for entry in temp_stack.take(initial_length) {
                        result_buffer.push(entry);
                        input_stack.next();
                    }
                    result.push(QueryValue::Array(result_buffer));
                }
                'B' => {
                    let v = input_stack.next().unwrap();
                    result.push(QueryValue::Number(utils::bcd_to_int(v as i32) as i64));
                }
                'W' => {
                    let v = (input_stack.next().unwrap() as i32) << 8
                        | input_stack.next().unwrap() as i32;
                    result.push(QueryValue::Number(utils::bcd_to_int(v) as i64));
                }
                _ => return Err(QueryError::UnrecognizedChar(character)),
            }
            continue;
        }
        if character == '%' {
            assert_eq!(half, None);
            escaped = true;
            continue;
        }
        if character == ' ' {
            continue;
        }
        if half.is_none() {
            half = Some(character);
        } else {
            let input_value = input_stack.next().unwrap();
            let format_value =
                u8::from_str_radix(&String::from_iter([half.unwrap(), character]), 16).unwrap();
            if format_value != input_value {
                let i = initial_length - input_stack.len() - 1;
                return Err(QueryError::InputMismatch {
                    index: i,
                    expected: format_value,
                    actual: input_value,
                    format_string: format,
                });
            }
            half = None;
        }
    }

    assert_eq!(input_stack.len(), 0);
    Ok(result)
}
