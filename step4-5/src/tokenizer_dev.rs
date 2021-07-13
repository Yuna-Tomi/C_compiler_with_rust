use crate::{exit_eprintln};

#[derive(Debug, PartialEq)]
pub enum Tokenkind {
	TK_HEAD, // 先頭にのみ使用するkind
	TK_RESERVED, // 記号
	TK_NUM, // 整数トークン
	TK_EOF, // 入力終わり
}

// Boxを利用すればポインタを使える
pub struct Token {
	pub kind: Tokenkind,
	pub val: Option<i32>,  
	pub body: Option<String>,
	pub next: Option<Box<Token>>,
	
}

// 構造体にStringをうまく持たせるためのnewメソッド
impl Token {
	fn new(kind: Tokenkind, body: impl Into<String>) -> Token {
		let body = body.into();
		match kind {
			Tokenkind::TK_HEAD => {
				Token {kind: kind, val: None, body: None, next: None}
			},
			Tokenkind::TK_NUM => {
				// TK_NUMと共に数字以外の値が渡されることはないものとして、unwrapで処理
				let val = body.parse::<i32>().unwrap();
				Token {kind: kind, val: Some(val), body: Some(body), next: None}
			},
			Tokenkind::TK_RESERVED => {
				Token {kind: kind, val: None, body: Some(body), next: None}
			},
			Tokenkind::TK_EOF => {
				Token {kind: kind, val: None, body: Some("EOF".to_string()), next: None}
			},
		}
	}
}


// 入力文字列のトークナイズ
pub fn tokenize(string: String) -> Option<Box<Token>> {
	// Box<Token>を使って読み進める
	let head = Token::new(Tokenkind::TK_HEAD, "");
	let mut token_ptr: Box<Token> = Box::new(head);
	head.next = Some(token_ptr);

	// StringをVec<char>としてlookat(インデックス)を進めることでトークナイズを行う(*char p; p++;みたいなことは気軽にできない)
	let len: usize = string.len();
	let mut lookat: usize = 0;
	let mut c: char;
	let string: Vec<char> = string.as_str().chars().collect::<Vec<char>>(); 


	while lookat < len {
		// 余白をまとめて飛ばす。streamを最後まで読んだならbreakする。
		match skipspace(&string, &mut lookat) {
			Ok(()) => {},
			Err(()) => {break;}
		}

		c = string[lookat];
		if c == '+' || c == '-' {
			token_ptr.next = Some(Box::new(Token::new(Tokenkind::TK_RESERVED, c)));
			token_ptr = token_ptr.next.unwrap();

			lookat += 1;
			continue;
		}

		// 数字ならば、数字が終わるまでを読んでトークンを生成
		if isdigit(c) {
			let num = strtol(&string, &mut lookat);
			token_ptr.next = Some(Box::new(Token::new(Tokenkind::TK_NUM, num.to_string())));
			token_ptr = token_ptr.next.unwrap();

			continue;
		}
	}

	token_ptr.next = Some(Box::new(Token::new(Tokenkind::TK_EOF, "")));

	head.next
}

// 空白を飛ばして読み進める
fn skipspace(string: &Vec<char>, index: &mut usize) -> Result<(), ()> {
	let limit = string.len();

	// 既にEOFだったならErrを即返す
	if *index >= limit {
		return Err(());
	}

	// 空白でなくなるまで読み進める
	while string[*index] == ' ' {
		*index += 1;
		if *index >= limit {
			return Err(());
		}
	}


	Ok(())
}

// 数字かどうかを判別する
fn isdigit(c: char) -> bool{
	c >= '0' && c <=  '9'
}

// 数字を読みつつindexを進める
fn strtol(string: &Vec<char>, index: &mut usize) -> u32 {
	let mut c = string[*index];
	let mut val = 0;
	let limit = string.len();

	// 数字を読む限りu32として加える
	while isdigit(c) {
		val = val * 10 + (c.to_digit(10).unwrap() - '0'.to_digit(10).unwrap());
		*index += 1;

		// 最後に到達した場合は処理を終える
		if *index >= limit {
			return val;
		}
		c = string[*index];
	} 

	val
}



// 次のトークンが数字であることを期待して次のトークンを読む関数
pub fn expect_number(token_ptr: &mut Box<Token>) -> i32 {
	if token_ptr.kind != Tokenkind::TK_NUM {
		exit_eprintln!("数字であるべき位置で数字以外の文字\"{}\"が発見されました。", token_ptr.body.as_ref().unwrap());
	}
	let val = token_ptr.val.unwrap();

	// 参照を次のトークンに移す(この時点でEOFでないのでnext.unwrap()して良い)
	*token_ptr = token_ptr.next.unwrap();
	
	val
}

// 期待する次のトークンを(文字列で)指定して読む関数(失敗するとexitする)
pub fn expect(token_ptr: &mut Box<Token>, op: &str) {

	if token_ptr.kind != Tokenkind::TK_RESERVED{
		exit_eprintln!("予約されていないトークン\"{}\"が発見されました。", token_ptr.body.as_ref().unwrap());
	}
	if token_ptr.body.as_ref().unwrap() != op {
		exit_eprintln!("\"{}\"を期待した位置で\"{}\"が発見されました。", op, token_ptr.body.as_ref().unwrap());
	}

	// 参照を次のトークンに移す(この時点でEOFでないのでnext.unwrap()して良い)
	*token_ptr = token_ptr.next.unwrap();
}


// 期待する次のトークンを(文字列で)指定して読む関数(失敗するとfalseを返す)
pub fn consume(token_ptr: &mut Box<Token>, op: &str) -> bool {
	if token_ptr.kind != Tokenkind::TK_RESERVED || token_ptr.body.as_ref().unwrap() != op {
		return false;
	}

	// 参照を次のトークンに移す(この時点でEOFでないのでnext.unwrap()して良い)
	*token_ptr = token_ptr.next.unwrap();

	true
}


// EOFかどうかを判断する関数
pub fn at_eof(token_ptr: &Box<Token>) -> bool{
	token_ptr.kind == Tokenkind::TK_EOF
}



