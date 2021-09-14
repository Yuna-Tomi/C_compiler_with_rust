// 再帰下降構文のパーサ
use crate::tokenizer::{Token, Tokenkind, consume, consume_kind, expect, expect_number, expect_ident, is_ident, at_eof};
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::fmt::{Formatter, Display, Result};

static LOCALS: Lazy<Mutex<HashMap<String, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));
pub static LVAR_MAX_OFFSET: Lazy<Mutex<usize>> = Lazy::new(|| Mutex::new(0));

#[derive(Debug, PartialEq)]
pub enum Nodekind {
	DefaultNd, // defalut
	AddNd, // '+'
	SubNd, // '-'
	MulNd, // '*'
	DivNd, // '/'
	AssignNd, // '='
	LvarNd, // 左辺値
	NumNd, // 数値
	EqNd, // "=="
	NEqNd, // "!="
	GThanNd, // '>'
	GEqNd, // ">="
	LThanNd, // '<'
	LEqNd, // "<="
	IfNd, // if
	// ElseNd, // else
	ForNd, // for
	WhileNd, // while
	ReturnNd, // return
}

pub struct Node {
	pub kind: Nodekind, // Nodeの種類

	// プロパティとなる数値
	pub val: Option<i32>,
	pub offset: Option<usize>,// ベースポインタからのオフセット(ローカル変数時のみ)

	// 通常ノード(計算式評価)用の左右ノード
	pub left: Option<Rc<RefCell<Node>>>,
	pub right: Option<Rc<RefCell<Node>>>,

	// for (init; enter; routine) branch, if (enter) branch else els, while(enter) branch 
	pub init: Option<Rc<RefCell<Node>>>,
	pub enter: Option<Rc<RefCell<Node>>>, 
	pub routine: Option<Rc<RefCell<Node>>>, 
	pub branch: Option<Rc<RefCell<Node>>>,
	pub els: Option<Rc<RefCell<Node>>>,

}

// 初期化を簡単にするためにデフォルトを定義
impl Default for Node {
	fn default() -> Node {
		Node { kind: Nodekind::DefaultNd, val: None, offset: None, left: None, right: None, init: None, enter: None, routine: None, branch: None, els: None, }
	}
}

static REP_NODE:usize = 40;
impl Display for Node {
	fn fmt(&self, f:&mut Formatter) -> Result {

		let mut s = format!("{}\n", "-".to_string().repeat(REP_NODE));
		s = format!("{}Nodekind : {:?}\n", s, self.kind);

		if let Some(e) = self.val.as_ref() {
			s = format!("{}val: {}\n", s, e);
		}
		
		if let Some(e) = self.offset.as_ref() {
			s = format!("{}offset: {}\n", s, e);
		} 

		if let Some(e) = self.left.as_ref() {
			s = format!("{}left: exist(kind:{:?})\n", s, e.borrow().kind);
		} 

		if let Some(e) = self.right.as_ref() {
			s = format!("{}right: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		if let Some(e) = self.init.as_ref() {
			s = format!("{}init: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		if let Some(e) = self.enter.as_ref() {
			s = format!("{}enter: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		if let Some(e) = self.routine.as_ref() {
			s = format!("{}routine: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		if let Some(e) = self.branch.as_ref() {
			s = format!("{}branch: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		if let Some(e) = self.els.as_ref() {
			s = format!("{}els: exist(kind:{:?})\n", s, e.borrow().kind);
		}

		write!(f, "{}", s)
	}
}


// ノードの作成
fn new_node_calc(kind: Nodekind, left: Rc<RefCell<Node>>, right: Rc<RefCell<Node>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(
		Node {
			kind: kind,
			left: Some(left), 
			right: Some(right),
			.. Default::default()
		}
	))
}

// 数字に対応するノード
fn new_node_num(val: i32) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(
		Node {
			kind: Nodekind::NumNd,
			val: Some(val),
			.. Default::default()
		}
	))
}

// 左辺値(今のうちはローカル変数)に対応するノード(現在は1文字のみ)
fn new_node_lvar(name: impl Into<String>) -> Rc<RefCell<Node>> {
	let name: String = name.into();
	let offset;


	// デッドロック回避のため、フラグを用意してmatch内で再度LOCALS(<変数名, オフセット>のHashMap)にアクセスしないようにする
	let mut not_found: bool = false;
	match LOCALS.lock().unwrap().get(&name) {
		Some(_offset) => {
			offset = *_offset;
		}, 
		// 見つからなければオフセットの最大値を伸ばす
		None => {
			*LVAR_MAX_OFFSET.lock().unwrap() += 8; 
			offset = *LVAR_MAX_OFFSET.lock().unwrap();
			not_found = true;
		}
	}

	if not_found {
		LOCALS.lock().unwrap().insert(name, offset); 
	}
	
	Rc::new(RefCell::new(
		Node {
			kind: Nodekind::LvarNd,
			offset: Some(offset),
			.. Default::default()
		}

	))
}


// 生成規則: program = stmt*
pub fn program(token_ptr: &mut Rc<RefCell<Token>>) -> Vec<Rc<RefCell<Node>>> {
	let mut statements :Vec<Rc<RefCell<Node>>> = Vec::new();
	while !at_eof(token_ptr) {
		statements.push(stmt(token_ptr));
	}

	statements
}


// 生成規則: stmt = 
// expr ";" | 
// "{" stmt* "}" | 
// "if" "(" expr ")" stmt ("else" stmt)? | ...(今はelse ifは実装しない)
// "while" "(" expr ")" stmt | 
// "for" "(" expr? ";" expr? ";" expr? ")" stmt |
// "return" expr ";"
// まだブロックには対応していない(一気に実装してごちゃつくのを防ぐため
fn stmt(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr: Rc<RefCell<Node>>;

	// if consume(token_ptr, ";") {return node_ptr;}
	if consume(token_ptr, "{") {
		// PENDING
		let childs: Vec<Option<RefCell<Node>>> = vec![];
		node_ptr = Rc::new(RefCell::new(
			Node{
				childs: childs,
				..Default::default()
			}

		));

	} else if consume(token_ptr, "if") {
		expect(token_ptr, "(");
		let enter= Some(expr(token_ptr));

		expect(token_ptr, ")");
		let branch = Some(stmt(token_ptr));

		if consume(token_ptr, "else") {
			let els = Some(stmt(token_ptr));
			node_ptr = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::IfNd,
				enter: enter,
				branch: branch,
				els: els,
				..Default::default()
				}
			));
		} else {
			node_ptr = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::IfNd,
				enter: enter,
				branch: branch,
				..Default::default()
				}
			));
		}
		

	} else if consume(token_ptr, "while") {
		expect(token_ptr, "(");
		let enter = Some(expr(token_ptr));
		expect(token_ptr, ")");

		let branch = Some(stmt(token_ptr));

		node_ptr = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::WhileNd,
				enter: enter,
				branch: branch,
				..Default::default()
			}
		));

	} else if consume(token_ptr, "for") {
		expect(token_ptr, "(");

		let init: Option<Rc<RefCell<Node>>>;
		let enter: Option<Rc<RefCell<Node>>>;
		let routine: Option<Rc<RefCell<Node>>>;
		let branch: Option<Rc<RefCell<Node>>>;

		if consume(token_ptr, ";") {
			// consumeできた場合exprが何も書かれていないことに注意
			init = None;
		} else {
			init = Some(expr(token_ptr));
			expect(token_ptr, ";");
		}

		// concumeの条件分岐について同上なので注意
		if consume(token_ptr, ";") {
			enter = None;
		} else {
			enter = Some(expr(token_ptr));
			expect(token_ptr, ";")
		}

		if consume(token_ptr, ")") {
			routine = None;
		} else {
			routine = Some(expr(token_ptr));
			expect(token_ptr, ")")
		}

		branch = Some(stmt(token_ptr));
		
		node_ptr = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::ForNd,
				init: init,
				enter: enter,
				routine: routine,
				branch: branch,
				..Default::default()
			}
		));

	} else if consume_kind(token_ptr, Tokenkind::ReturnTk) {
		// ReturnNdはここでしか生成しないため、ここにハードコードする
		node_ptr = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::ReturnNd,
				left: Some(expr(token_ptr)),
				..Default::default()
			}
		));
		expect(token_ptr, ";");

	} else {
		node_ptr = expr(token_ptr);
		expect(token_ptr, ";");

	}

	node_ptr
}

// 生成規則: expr = assign
pub fn expr(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	assign(token_ptr)
}

// 生成規則: assign = equality ("=" assign)?
fn assign(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {

	let mut node_ptr = equality(token_ptr);
	if consume(token_ptr, "=") {
		node_ptr = new_node_calc(Nodekind::AssignNd, node_ptr,  assign(token_ptr));
	}
	
	node_ptr
}

// 生成規則: equality = relational ("==" relational | "!=" relational)?
pub fn equality(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {

	let mut node_ptr = relational(token_ptr);
	if consume(token_ptr, "==") {
		node_ptr = new_node_calc(Nodekind::EqNd, node_ptr, relational(token_ptr));

	} else if consume(token_ptr, "!=") {
		node_ptr = new_node_calc(Nodekind::NEqNd, node_ptr, relational(token_ptr));
	}

	node_ptr
}

// 生成規則: relational = add ("<" add | "<=" add | ">" add | ">=" add)*
fn relational(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr = add(token_ptr);

	loop {
		if consume(token_ptr, "<") {
			node_ptr = new_node_calc(Nodekind::LThanNd, node_ptr, add(token_ptr));

		} else if consume(token_ptr, "<=") {
			node_ptr = new_node_calc(Nodekind::LEqNd, node_ptr, add(token_ptr));

		} else if consume(token_ptr, ">") {
			node_ptr = new_node_calc(Nodekind::GThanNd, node_ptr, add(token_ptr));

		} else if consume(token_ptr, ">=") {
			node_ptr = new_node_calc(Nodekind::GEqNd, node_ptr, add(token_ptr));

		} else{
			break;
		}
	}


	node_ptr

}

// 生成規則: add = mul ("+" mul | "-" mul)*
pub fn add(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr = mul(token_ptr);

	loop {
		if consume(token_ptr, "+") {
			node_ptr = new_node_calc(Nodekind::AddNd, node_ptr, mul(token_ptr));

		} else if consume(token_ptr, "-") {
			node_ptr = new_node_calc(Nodekind::SubNd, node_ptr, mul(token_ptr));

		} else {
			break;
		}
	}

	node_ptr
}

// 生成規則: mul = unary ("*" unary | "/" unary)*
fn mul(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr = unary(token_ptr);

	loop {
		if consume(token_ptr, "*") {
			node_ptr = new_node_calc(Nodekind::MulNd, node_ptr, unary(token_ptr));

		} else if consume(token_ptr, "/") {
			node_ptr = new_node_calc(Nodekind::DivNd, node_ptr, unary(token_ptr));

		} else {
			break;
		}
	}

	node_ptr
}


// 生成規則: unary = ("+" | "-")? primary
fn unary(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr;

	if consume(token_ptr, "+") {
		node_ptr = primary(token_ptr);

	} else if consume(token_ptr, "-") {
		// 単項演算のマイナスは0から引く形にする。
		node_ptr = new_node_calc(Nodekind::SubNd, new_node_num(0), primary(token_ptr));

	} else {
		node_ptr = primary(token_ptr);
	}

	node_ptr
}



// 生成規則: primary = num | ident | "(" expr ")"
fn primary(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr;

	if consume(token_ptr, "(") {
		node_ptr = expr(token_ptr);

		expect(token_ptr, ")");

	} else if is_ident(token_ptr) {
		node_ptr = new_node_lvar(expect_ident(token_ptr));

	} else {
		node_ptr = new_node_num(expect_number(token_ptr));

	}

	node_ptr
}

mod tests {
	use super::*;
	use crate::tokenizer::tokenize;

	static REP: usize = 40;

	fn search_tree(tree: &Rc<RefCell<Node>>) {
		let node: &Node = &*(*tree).borrow();
		println!("{}", node);

		if node.left.is_some() {search_tree(node.left.as_ref().unwrap());}
		if node.right.is_some() {search_tree(node.right.as_ref().unwrap());}
		if node.init.is_some() {search_tree(node.init.as_ref().unwrap());}
		if node.enter.is_some() {search_tree(node.enter.as_ref().unwrap());}
		if node.routine.is_some() {search_tree(node.routine.as_ref().unwrap());}
		if node.branch.is_some() {search_tree(node.branch.as_ref().unwrap());}
		if node.els.is_some() {search_tree(node.els.as_ref().unwrap());}
	}


	#[test]
	fn test_display() {
		println!("test_display{}", "-".to_string().repeat(REP));
		let node = new_node_num(0);
		println!("{}", (*node).borrow());
	}


	#[test]
	fn test_for() {
		println!("test_for{}", "-".to_string().repeat(REP));
		let equation = "
			sum = 10;
			sum = sum + i;
			for (i = 1 ; i < 10; i = i + 1) sum = sum +i;
			sum;
		".to_string();
		let mut token_ptr = tokenize(equation);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, "-".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn test_while() {
		println!("test_while{}", "-".to_string().repeat(REP));
		let equation = "
			sum = 10;
			while(sum > 0) sum = sum - 1;
			sum;
		".to_string();
		let mut token_ptr = tokenize(equation);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, "-".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn test_if() {
		println!("test_while{}", "-".to_string().repeat(REP));
		let equation = "
			i = 10;
			if (i == 10) i = i / 5;
			if (i == 2) i = i + 5; else i = i / 5;
			i;
		".to_string();
		let mut token_ptr = tokenize(equation);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, "-".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}


	#[test]
	fn test_combination() {
		println!("test_combination{}", "-".to_string().repeat(REP));
		let equation = "
			i = 10;
			if (i == 10) i = i / 5;
			if (i == 2) i = i + 5; else i = i / 5;
			i;
		".to_string();
		let mut token_ptr = tokenize(equation);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, "-".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn test_block() {
		println!("test_combination{}", "-".to_string().repeat(REP));
		let equation = "
			for( i = 10; ;  ) {i = i + 1;}
			{}
			{i = i + 1; }
			return 10
		".to_string();
		let mut token_ptr = tokenize(equation);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, "-".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

}