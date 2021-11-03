// 再帰下降構文のパーサ
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::{
	node::{Node, Nodekind},
	token::{Token, Tokenkind},
	tokenizer::{consume, consume_ident, consume_kind, consume_type, expect, expect_ident, expect_number, expect_type, at_eof},
	typecell::{Type, TypeCell},
	exit_eprintln, error_with_token, error_with_node
};

static LOCALS: Lazy<Mutex<HashMap<String, (usize, TypeCell)>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static ARGS_COUNTS: Lazy<Mutex<HashMap<String, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static LVAR_MAX_OFFSET: Lazy<Mutex<usize>> = Lazy::new(|| Mutex::new(0));

// 2つ子を持つ汎用ノード
fn _binary(kind: Nodekind, left: Rc<RefCell<Node>>, right: Rc<RefCell<Node>>, token: Option<Rc<RefCell<Token>>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(Node {kind: kind, token: token, left: Some(left), right: Some(right), .. Default::default()}))
}

fn new_binary(kind: Nodekind, left: Rc<RefCell<Node>>, right: Rc<RefCell<Node>>, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	_binary(kind, left, right, Some(token_ptr))
}

macro_rules! tmp_binary {
	($($args:tt)*) => {
		_binary($($args)*, None)
	};
}

// 1つ子を持つ汎用ノード
fn _unary(kind: Nodekind, left: Rc<RefCell<Node>>, token: Option<Rc<RefCell<Token>>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(Node {kind: kind, token: token, left: Some(left), .. Default::default()}))
}

fn new_unary(kind: Nodekind, left: Rc<RefCell<Node>>, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	_unary(kind, left, Some(token_ptr))
}

macro_rules! tmp_unary {
	($($args:tt)*) => {
		_unary($($args)*, None)
	};
}

// 数字に対応するノード
fn _num(val: i32, token: Option<Rc<RefCell<Token>>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(Node {kind: Nodekind::NumNd, token: token, val: Some(val), .. Default::default()}))
}

fn new_num(val: i32, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	_num(val, Some(token_ptr))
}

macro_rules! tmp_num {
	($num: expr) => {
		_num($num, None)
	};
}

// 左辺値(今のうちはローカル変数)に対応するノード: += などの都合で無名の変数を生成する場合があるため、token は Option で受ける
fn _lvar(name: impl Into<String>, token: Option<Rc<RefCell<Token>>>, typ: Option<TypeCell>) -> Rc<RefCell<Node>> {
	let name: String = name.into();
	let offset;

	// デッドロック回避のため、フラグを用意してmatch内で再度LOCALS(<変数名, オフセット>のHashMap)にアクセスしないようにする
	let mut not_found: bool = false;
	let mut locals = LOCALS.try_lock().unwrap();
	match locals.get(&name) {
		Some((offset_,_)) => {
			offset = *offset_;
		}, 
		// 見つからなければオフセットの最大値を伸ばす
		None => {
			*LVAR_MAX_OFFSET.try_lock().unwrap() += 8; 
			offset = *LVAR_MAX_OFFSET.try_lock().unwrap();
			not_found = true;
		}
	}

	if not_found {
		// typ に渡されるのは Option だが LOCALS に保存するのは生の TypeCell なので let Some で分岐
		if let Some(typ_) = typ.clone() {
			locals.insert(name, (offset, typ_)); 
		} else {
			locals.insert(name, (offset, TypeCell::default()));
		}
	}
	
	Rc::new(RefCell::new(Node {kind: Nodekind::LvarNd, typ: typ, token: token, offset: Some(offset), .. Default::default()}))
}

fn new_lvar(name: impl Into<String>, token_ptr: Rc<RefCell<Token>>, typ: TypeCell) -> Rc<RefCell<Node>> {
	_lvar(name, Some(token_ptr), Some(typ))
}

macro_rules! tmp_lvar {
	() => {
		_lvar("", None, None)
	};
}

// ブロックのノード
fn new_block(children: Vec<Option<Rc<RefCell<Node>>>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(Node {kind: Nodekind::BlockNd, children: children, ..Default::default()}))
}

// 制御構文のためのノード
fn new_ctrl(kind: Nodekind,
			init: Option<Rc<RefCell<Node>>>,
			enter: Option<Rc<RefCell<Node>>>,
			routine: Option<Rc<RefCell<Node>>>,
			branch: Option<Rc<RefCell<Node>>>,
			els: Option<Rc<RefCell<Node>>>) -> Rc<RefCell<Node>> {
	if ![Nodekind::IfNd, Nodekind::ForNd, Nodekind::WhileNd].contains(&kind){
		exit_eprintln!("new_ctrl: 制御構文ではありません。");
	}
	Rc::new(RefCell::new(Node{kind: kind, init: init, enter: enter, routine: routine, branch: branch, els: els, ..Default::default()}))
}

// 関数呼び出しのノード
fn new_func(name: String, args: Vec<Option<Rc<RefCell<Node>>>>, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	Rc::new(RefCell::new(Node{kind: Nodekind::FuncNd, token: Some(token_ptr), name: Some(name), args: args, ..Default::default()}))
}

// TODO
// 型を構文木全体に対して設定する関数 (ここで cast なども行う？)
fn confirm_type(node: &Rc<RefCell<Node>>) {
	if let Some(_) = &node.borrow().typ { return; }

	if let Some(n) = &node.borrow().left { confirm_type(n); }
	if let Some(n) = &node.borrow().right { confirm_type(n); }

	let kind: Nodekind = node.borrow().kind;
	match kind {
		Nodekind::NumNd => { let _ = node.borrow_mut().typ.insert(TypeCell::new(Type::Int)); }
		Nodekind::AddrNd => {
			// & は変数やそのポインタにのみ可能であるため、このタイミングで left をチェックして弾くことができる
			let mut node = node.borrow_mut();
			let typ: TypeCell;
			{
				let left = node.left.as_ref().unwrap().borrow();
				if ![Nodekind::DerefNd, Nodekind::LvarNd].contains(&left.kind) {
					error_with_node!("\"&\" では変数として宣言された値のみ参照ができます。", &node);
				}
				typ = node.left.as_ref().unwrap().borrow().typ.as_ref().unwrap().clone();
			}
			let ptr_end = if typ.typ == Type::Ptr { typ.ptr_end.clone() } else { Some(typ.typ) };
			let _ = node.typ.insert(
				TypeCell { typ: Type::Ptr, ptr_end: ptr_end, chains: typ.chains+1 }
			);
		}
		Nodekind::DerefNd => {
			let mut node = node.borrow_mut();
			let typ: TypeCell;
			{
				typ = node.left.as_ref().unwrap().borrow().typ.as_ref().unwrap().clone();
			}
			if let Some(end) = &typ.ptr_end {
				// left がポインタ型だということなので、 chains は必ず正であり、1ならば参照外し後は値に、そうでなければポインタになることに注意
				let (ptr_end, new_typ) = if typ.chains > 1 { (Some(*end), Type::Ptr) } else { (None, typ.typ) };
				let _ = node.typ.insert(
					TypeCell { typ: new_typ, ptr_end: ptr_end, chains: typ.chains-1 }
				);
			} else {
				error_with_node!("\"*\"ではポインタの参照を外すことができますが、型\"{}\"が指定されています。", &node, typ.typ);
			}
		}
		Nodekind::AddNd | Nodekind::SubNd => {
			// TODO: ポインタ演算のキャスト処理
			// let mut node = node;
		}
		// Nodekind::MulNd => {}
		// Nodekind::EqNd | Nodekind::NEqNd | Nodekind::GThanNd | Nodekind::GEqNd => {}
		Nodekind::CommaNd => {
			// x, y の評価は y になるため、型も y のものを引き継ぐ
			let mut node = node.borrow_mut();
			let typ: TypeCell;
			{
				typ = node.right.as_ref().unwrap().borrow().typ.as_ref().unwrap().clone();
			}
			let _ = node.typ.insert(
				TypeCell { typ: typ.typ, ptr_end: typ.ptr_end.clone(), chains: typ.chains }
			);
		}
		_ => {}
	}
}


// 生成規則:
// type = "int"

// 生成規則:
// func-args = type ident ("," type ident)* | null
fn func_args(token_ptr: &mut Rc<RefCell<Token>>) -> Vec<Option<Rc<RefCell<Node>>>> {
	let mut args: Vec<Option<Rc<RefCell<Node>>>> = vec![];
	let mut argc: usize = 0;
	if let Some(typ) = consume_type(token_ptr) { // 型宣言があれば、引数ありと判断
		let ptr = token_ptr.clone();
		let name: String = expect_ident(token_ptr);
		args.push(Some(new_lvar(name, ptr, typ)));
		argc += 1;

		loop {
			if !consume(token_ptr, ",") {break;}
			if argc >= 6 {
				exit_eprintln!("現在7つ以上の引数はサポートされていません。");
			}
			let typ = expect_type(token_ptr); // 型宣言の読み込み
			let ptr = token_ptr.clone();
			let name: String = expect_ident(token_ptr);
			args.push(Some(new_lvar(name, ptr, typ)));
			argc += 1;
		}
	} else {
		// エラーメッセージがわかりやすくなるように分岐する
		let ptr = token_ptr.clone();
		if let Some(_) = consume_ident(token_ptr) {
			error_with_token!("型指定が必要です。", &*ptr.borrow());
		}
	}
	args
}

// 生成規則: 
// program = (type ident "(" func-args ")" "{" stmt* "}")*
pub fn program(token_ptr: &mut Rc<RefCell<Token>>) -> Vec<Rc<RefCell<Node>>> {
	let mut globals : Vec<Rc<RefCell<Node>>> = Vec::new();

	while !at_eof(token_ptr) {
		// トップレベル(グローバルスコープ)では、現在は関数宣言のみができる
		let mut statements : Vec<Rc<RefCell<Node>>> = Vec::new();

		let _ = expect_type(token_ptr); // 型宣言の読み込み
		let ptr =  token_ptr.clone();
		let func_name = expect_ident(token_ptr);
		if ARGS_COUNTS.try_lock().unwrap().contains_key(&func_name) {
			error_with_token!("{}: 重複した関数宣言です。", &*ptr.borrow(), func_name);
		}
		expect(token_ptr, "(");
		// 引数を6つまでサポート
		let args: Vec<Option<Rc<RefCell<Node>>>> = func_args(token_ptr);

		// 引数の数をチェックするためにマップに保存
		ARGS_COUNTS.try_lock().unwrap().insert(func_name.clone(), args.len());
		expect(token_ptr, ")");
		

		let mut has_return : bool = false;
		expect(token_ptr, "{");
		while !consume(token_ptr, "}") {
			has_return |= (**token_ptr).borrow().kind == Tokenkind::ReturnTk; // return がローカルの最大のスコープに出現するかどうかを確認 (ブロックでネストされていると対応できないのが難点…)
			statements.push(stmt(token_ptr));
		}

		if !has_return {
			statements.push(tmp_unary!(Nodekind::ReturnNd, tmp_num!(0)));
		}

		let global = Rc::new(RefCell::new(
			Node {
				kind: Nodekind::FuncDecNd,
				token: Some(ptr),
				name: Some(func_name),
				args: args,
				stmts: Some(statements),
				max_offset: Some(*LVAR_MAX_OFFSET.try_lock().unwrap()),
				..Default::default()
			}
		));
		// 関数宣言が終わるごとにローカル変数の管理情報をクリア(offset や name としてノードが持っているのでこれ以上必要ない)
		LOCALS.try_lock().unwrap().clear();
		*LVAR_MAX_OFFSET.try_lock().unwrap() = 0;

		globals.push(global);
	}
	
	globals
}

// 生成規則:
// declare = ident ("," ident)* ";"
fn declare(token_ptr: &mut Rc<RefCell<Token>>, typ: TypeCell) -> Rc<RefCell<Node>> {
	let ptr = token_ptr.clone();
	let name = expect_ident(token_ptr);
	let mut node_ptr = new_lvar(name, ptr, typ.clone());
	loop {
		let ptr_com = token_ptr.clone();
		if !consume(token_ptr, ",") { break; }
		let ptr = token_ptr.clone();
		let name = expect_ident(token_ptr);
		node_ptr = new_binary(Nodekind::CommaNd, node_ptr, new_lvar(name, ptr, typ.clone()), ptr_com);
	}
	expect(token_ptr,";");
	
	node_ptr
}


// 生成規則:
// stmt = expr? ";"
//		| type declare
//		| "{" stmt* "}" 
//		| "if" "(" expr ")" stmt ("else" stmt)?
//		| ...(今はelse ifは実装しない)
//		| "while" "(" expr ")" stmt
//		| "for" "(" expr? ";" expr? ";" expr? ")" stmt
//		| "return" expr? ";"
fn stmt(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let ptr = token_ptr.clone();

	if consume(token_ptr, ";") {
		tmp_num!(0)
	} else if let Some(typ) = consume_type(token_ptr) {
		declare(token_ptr, typ)
	} else if consume(token_ptr, "{") {
		let mut children: Vec<Option<Rc<RefCell<Node>>>> = vec![];
		loop {
			if !consume(token_ptr, "}") {
				if at_eof(token_ptr) {exit_eprintln!("\'{{\'にマッチする\'}}\'が見つかりません。");}
				children.push(Some(stmt(token_ptr)));
			} else {
				break;
			}
		}
		new_block(children)

	} else if consume(token_ptr, "if") {
		expect(token_ptr, "(");
		let enter= Some(expr(token_ptr));
		expect(token_ptr, ")");

		let branch = Some(stmt(token_ptr));
		let els = if consume(token_ptr, "else") {Some(stmt(token_ptr))} else {None};
		
		new_ctrl(Nodekind::IfNd, None, enter, None, branch, els)

	} else if consume(token_ptr, "while") {
		expect(token_ptr, "(");
		let enter = Some(expr(token_ptr));
		expect(token_ptr, ")");

		let branch = Some(stmt(token_ptr));

		new_ctrl(Nodekind::WhileNd, None, enter, None, branch, None)

	} else if consume(token_ptr, "for") {
		expect(token_ptr, "(");
		// consumeできた場合exprが何も書かれていないことに注意
		let init: Option<Rc<RefCell<Node>>> =
		if consume(token_ptr, ";") {None} else {
			let init_ = Some(expr(token_ptr));
			expect(token_ptr, ";");
			init_
		};

		let enter: Option<Rc<RefCell<Node>>> =
		if consume(token_ptr, ";") {None} else {
			let enter_ = Some(expr(token_ptr));
			expect(token_ptr, ";");
			enter_
		};

		let routine: Option<Rc<RefCell<Node>>> = 
		if consume(token_ptr, ")") {None} else {
			let routine_ = Some(expr(token_ptr));
			// confirm_type(&routine_.as_ref().unwrap());
			expect(token_ptr, ")");
			routine_
		};

		let branch: Option<Rc<RefCell<Node>>> = Some(stmt(token_ptr));
		
		new_ctrl(Nodekind::ForNd, init, enter, routine, branch, None)

	} else if consume_kind(token_ptr, Tokenkind::ReturnTk) {
		// exprなしのパターン: 実質NumNd 0があるのと同じと捉えれば良い
		let left: Rc<RefCell<Node>> =  
		if consume(token_ptr, ";") {
			tmp_num!(0)
		} else {
			let left_: Rc<RefCell<Node>> = expr(token_ptr);
			confirm_type(&left_);
			expect(token_ptr, ";");
			left_
		};

		new_unary(Nodekind::ReturnNd, left, ptr)

	} else {
		let node_ptr: Rc<RefCell<Node>> = expr(token_ptr);
		expect(token_ptr, ";");
		node_ptr
	}
}

// 生成規則:
// expr = assign ("," expr)? 
pub fn expr(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr: Rc<RefCell<Node>> = assign(token_ptr);
	let ptr = token_ptr.clone();

	if consume(token_ptr, ",") {
		new_binary(Nodekind::CommaNd, node_ptr, expr(token_ptr), ptr)
	} else {
		node_ptr
	}
}

// 禁止代入(例えば x + y = 10; や x & y = 10; など)は generator 側で弾く
// 生成規則:
// assign = logor (assign-op assign)?
// assign-op = "="
//			| "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "^=" | "|="
//			| "<<=" | ">>="
fn assign(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr: Rc<RefCell<Node>> = logor(token_ptr);
	let ptr = token_ptr.clone();

	if consume(token_ptr, "=") {
		new_binary(Nodekind::AssignNd, node_ptr,  assign(token_ptr), ptr)	
	} else if consume(token_ptr, "+=") {
		assign_op(Nodekind::AddNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "-=") {
		assign_op(Nodekind::SubNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "*=") {
		assign_op(Nodekind::MulNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "/=") {
		assign_op(Nodekind::DivNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "%=") {
		assign_op(Nodekind::ModNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "&=") {
		assign_op(Nodekind::BitAndNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "^=") {
		assign_op(Nodekind::BitXorNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "|=") {
		assign_op(Nodekind::BitOrNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, "<<=") {
		assign_op(Nodekind::LShiftNd, node_ptr, assign(token_ptr), ptr)
	} else if consume(token_ptr, ">>=") {
		assign_op(Nodekind::RShiftNd, node_ptr, assign(token_ptr), ptr)
	} else {
		node_ptr
	} 
}

// a += b; -->  tmp = &a, *tmp = *tmp + b;
// AssignAddNd 的な Nodekind を導入して generator で add [a], b となるように直接処理する手もある
fn assign_op(kind: Nodekind, left: Rc<RefCell<Node>>, right: Rc<RefCell<Node>>, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	// 左右の型を確定させておく
	confirm_type(&left);
	confirm_type(&right);

	// プレーンな "=" の場合は単に通常通りのノード作成で良い
	if kind == Nodekind::AssignNd {
		new_binary(Nodekind::AssignNd, left,  right, token_ptr)
	} else {
		// tmp として通常は認められない無名の変数を使うことで重複を避ける
		let expr_left = tmp_binary!(
			Nodekind::AssignNd,
			tmp_lvar!(),
			tmp_unary!(Nodekind::AddrNd, left)
		);

		let expr_right = tmp_binary!(
			Nodekind::AssignNd,
			tmp_unary!(Nodekind::DerefNd, tmp_lvar!()),
			tmp_binary!(kind, tmp_unary!(Nodekind::DerefNd, tmp_lvar!()), right)
		);

		new_binary(Nodekind::CommaNd, expr_left, expr_right, token_ptr)
	}
}

// 生成規則:
// logor = logand ("||" logand)*
fn logor(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = logand(token_ptr);
	loop {
		let ptr = token_ptr.clone();
		if !consume(token_ptr, "||") { break; }
		node_ptr = new_binary(Nodekind::LogOrNd, node_ptr, logand(token_ptr), ptr);
	}

	node_ptr
}

// 生成規則:
// logand = bitor ("&&" bitor)*
fn logand(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = bitor(token_ptr);
	loop {
		let ptr = token_ptr.clone();
		if !consume(token_ptr, "&&") { break; }
		node_ptr = new_binary(Nodekind::LogAndNd, node_ptr, bitor(token_ptr), ptr);
	}

	node_ptr
}

// 生成規則:
// bitor = bitxor ("|" bitxor)*
fn bitor(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = bitxor(token_ptr);
	loop{
		let ptr = token_ptr.clone();
		if !consume(token_ptr, "|") { break; }
		node_ptr = new_binary(Nodekind::BitOrNd, node_ptr, bitxor(token_ptr), ptr);
	}

	node_ptr
}

// 生成規則:
// bitxor = bitand ("^" bitand)*
fn bitxor(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = bitand(token_ptr);
	loop{
		let ptr = token_ptr.clone();
		if !consume(token_ptr, "^") { break; }
		node_ptr = new_binary(Nodekind::BitXorNd, node_ptr, bitand(token_ptr), ptr);
	}

	node_ptr
}

// 生成規則:
// bitand = equality ("&" equality)*
fn bitand(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = equality(token_ptr);
	loop{
		let ptr = token_ptr.clone();
		if !consume(token_ptr, "&") { break; }
		node_ptr = new_binary(Nodekind::BitAndNd, node_ptr, equality(token_ptr), ptr);
	}

	node_ptr
}

// 生成規則:
// equality = relational ("==" relational | "!=" relational)?
pub fn equality(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr: Rc<RefCell<Node>> = relational(token_ptr);
	let ptr = token_ptr.clone();

	if consume(token_ptr, "==") {
		new_binary(Nodekind::EqNd, node_ptr, relational(token_ptr), ptr)
	} else if consume(token_ptr, "!=") {
		new_binary(Nodekind::NEqNd, node_ptr, relational(token_ptr), ptr)
	} else {
		node_ptr
	}
}

// 生成規則:
// relational = shift ("<" shift | "<=" shift | ">" shift | ">=" shift)*
fn relational(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = shift(token_ptr);

	loop {
		let ptr = token_ptr.clone();
		if consume(token_ptr, "<") {
			node_ptr = new_binary(Nodekind::LThanNd, node_ptr, shift(token_ptr), ptr);

		} else if consume(token_ptr, "<=") {
			node_ptr = new_binary(Nodekind::LEqNd, node_ptr, shift(token_ptr), ptr);

		} else if consume(token_ptr, ">") {
			node_ptr = new_binary(Nodekind::LThanNd, shift(token_ptr), node_ptr, ptr);

		} else if consume(token_ptr, ">=") {
			node_ptr = new_binary(Nodekind::LEqNd, shift(token_ptr), node_ptr, ptr);

		} else{
			break;
		}
	}

	node_ptr
}

// 生成規則:
// shift = add ("<<" add | ">>" add)*
pub fn shift(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = add(token_ptr);

	loop {
		let ptr = token_ptr.clone();
		if consume(token_ptr, "<<") {
			node_ptr = new_binary(Nodekind::LShiftNd, node_ptr, add(token_ptr), ptr);

		} else if consume(token_ptr, ">>") {
			node_ptr = new_binary(Nodekind::RShiftNd, node_ptr, add(token_ptr), ptr);

		} else {
			break;
		}
	}

	node_ptr
}

// 生成規則:
// add = mul ("+" mul | "-" mul)*
pub fn add(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = mul(token_ptr);

	loop {
		let ptr = token_ptr.clone();
		if consume(token_ptr, "+") {
			node_ptr = new_binary(Nodekind::AddNd, node_ptr, mul(token_ptr), ptr);

		} else if consume(token_ptr, "-") {
			node_ptr = new_binary(Nodekind::SubNd, node_ptr, mul(token_ptr), ptr);

		} else {
			break;
		}
	}

	node_ptr
}

// 生成規則:
// mul = unary ("*" unary | "/" unary | "%" unary)*
fn mul(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let mut node_ptr: Rc<RefCell<Node>> = unary(token_ptr);
	loop {
		let ptr = token_ptr.clone();
		if consume(token_ptr, "*") {
			node_ptr = new_binary(Nodekind::MulNd, node_ptr, unary(token_ptr), ptr);

		} else if consume(token_ptr, "/") {
			node_ptr = new_binary(Nodekind::DivNd, node_ptr, unary(token_ptr), ptr);

		} else if consume(token_ptr, "%") {
			node_ptr = new_binary(Nodekind::ModNd, node_ptr, unary(token_ptr), ptr);

		} else {
			break;
		}
	}

	node_ptr
}

// TODO: *+x; *-y; みたいな構文を禁止したい
// !+x; や ~-y; は valid
// unary = tailed 
//		| ("+" | "-")? unary
//		| ("!" | "~")? unary
//		| ("*" | "&")? unary 
//		| ("++" | "--")? unary 
fn unary(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let ptr = token_ptr.clone();

	if consume(token_ptr, "~") {
		new_unary(Nodekind::BitNotNd, unary(token_ptr), ptr)
	} else if consume(token_ptr, "!") {
		new_unary(Nodekind::LogNotNd, unary(token_ptr), ptr)
	} else if consume(token_ptr, "*") {
		let node_ptr = unary(token_ptr);
		// confirm_type(&node_ptr);
		new_unary(Nodekind::DerefNd, node_ptr, ptr)
	} else if consume(token_ptr, "&") {
		let node_ptr = unary(token_ptr);
		// confirm_type(&node_ptr);
		new_unary(Nodekind::AddrNd, node_ptr, ptr)
	} else if consume(token_ptr, "+") {
		// 単項演算子のプラスは0に足す形にする。こうすることで &+var のような表現を generator 側で弾ける
		new_binary(Nodekind::AddNd, tmp_num!(0), primary(token_ptr), ptr)
	} else if consume(token_ptr, "-") {
		// 単項演算のマイナスは0から引く形にする。
		new_binary(Nodekind::SubNd, tmp_num!(0), primary(token_ptr), ptr)
	} else if consume(token_ptr, "++") {
		assign_op(Nodekind::AddNd, unary(token_ptr), tmp_num!(1), ptr)
	} else if consume(token_ptr, "--") {
		assign_op(Nodekind::SubNd, unary(token_ptr), tmp_num!(1), ptr)
	} else {
		tailed(token_ptr)
	}
}

// 生成規則:
// tailed = primary (primary-tail)?
// primary-tail = "++" | "--"
fn tailed(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let node_ptr: Rc<RefCell<Node>> = primary(token_ptr);
	let ptr = token_ptr.clone();

	if consume(token_ptr, "++") {
		inc_dec(node_ptr, true, false, ptr)

	} else if consume(token_ptr, "--") {
		inc_dec(node_ptr, false, false, ptr)

	} else {
		node_ptr
	}
}

fn inc_dec(node: Rc<RefCell<Node>>, is_inc: bool, is_prefix: bool, token_ptr: Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let kind = if is_inc { Nodekind::AddNd } else { Nodekind::SubNd };
	confirm_type(&node);

	if is_prefix {
		// ++i は (i+=1) として読み替えると良い
		assign_op(kind, node, tmp_num!(1), token_ptr)
	} else {
		// i++ は (i+=1)-1 として読み替えると良い
		let opposite_kind = if !is_inc { Nodekind::AddNd } else { Nodekind::SubNd };
		// この部分木でエラーが起きる際、部分木の根が token を持っている(Some)必要があることに注意
		new_binary(opposite_kind, assign_op(kind, node, tmp_num!(1), token_ptr.clone()), tmp_num!(1), token_ptr) 
	}
}

// 生成規則:
// params = assign ("," assign)* | null
fn params(token_ptr: &mut Rc<RefCell<Token>>) -> Vec<Option<Rc<RefCell<Node>>>> {
	let mut args: Vec<Option<Rc<RefCell<Node>>>> = vec![];
	if !consume(token_ptr, ")") {
		let arg = assign(token_ptr);
		// confirm_type(&arg);
		args.push(Some(arg));

		loop {
			if !consume(token_ptr, ",") {
				expect(token_ptr,")"); // 括弧が閉じないような書き方になっているとここで止まるため、if at_eof ~ のようなチェックは不要
				break;
			}
			let arg = assign(token_ptr);
			// confirm_type(&arg);
			args.push(Some(arg));
		}
	}
	args
}

// 生成規則: 
// primary = num
//			| ident ( "(" params ")" )?
//			| "(" expr ")"
fn primary(token_ptr: &mut Rc<RefCell<Token>>) -> Rc<RefCell<Node>> {
	let ptr = token_ptr.clone();

	if consume(token_ptr, "(") {
		let node_ptr: Rc<RefCell<Node>> = expr(token_ptr);
		expect(token_ptr, ")");

		node_ptr

	} else if let Some(name) = consume_ident(token_ptr) {

		if consume(token_ptr, "(") {
			let args:Vec<Option<Rc<RefCell<Node>>>> = params(token_ptr);
			// 本来、宣言されているかを contains_key で確認したいが、今は外部の C ソースとリンクさせているため、このコンパイラの処理でパースした関数に対してのみ引数の数チェックをするにとどめる。
			let declared: bool = ARGS_COUNTS.try_lock().unwrap().contains_key(&name);
			if declared  {
				let argc = *ARGS_COUNTS.try_lock().unwrap().get(&name).unwrap();
				if args.len() != argc { error_with_token!("\"{}\" の引数は{}個で宣言されていますが、{}個が渡されました。", &*ptr.borrow(), name, argc, args.len()); }
			}
			new_func(name, args, ptr)
		} else {
			let typ: TypeCell;
			{
				let locals = LOCALS.try_lock().unwrap();
				let declared: bool = locals.contains_key(&name);
				if !declared { error_with_token!("\"{}\" が定義されていません。", &*ptr.borrow(), name); }
				typ = locals.get(&name).as_ref().unwrap().1.clone();
			}
			new_lvar(name, ptr, typ.clone())
		}

	} else {
		new_num(expect_number(token_ptr), ptr)
	}
}


#[cfg(test)]
pub mod tests {
	use crate::tokenizer::tokenize;
	use crate::globals::{CODES, FILE_NAMES};
	use super::*;
	
	static REP: usize = 40;

	fn test_init(src: &str) {
		let mut src_: Vec<String> = src.split("\n").map(|s| s.to_string()+"\n").collect();
		FILE_NAMES.try_lock().unwrap().push("test".to_string());
		let mut code = vec!["".to_string()];
		code.append(&mut src_);
		CODES.try_lock().unwrap().push(code);
	}

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
		for child in &node.children {
			if child.is_some() {search_tree(child.as_ref().unwrap());}
		}
		for arg in &node.args {
			if arg.is_some() {search_tree(arg.as_ref().unwrap());}
		}
		if node.stmts.is_some() {
			for stmt_ in node.stmts.as_ref().unwrap() {
				search_tree(stmt_);
			}
		}
	}

	pub fn parse_stmts(token_ptr: &mut Rc<RefCell<Token>>) -> Vec<Rc<RefCell<Node>>> {
		let mut statements :Vec<Rc<RefCell<Node>>> = Vec::new();
		while !at_eof(token_ptr) {
			statements.push(stmt(token_ptr));
		}
		statements
	}

	#[test]
	fn basic_calc() {
		let src: &str = "
			int x, y, z;
			x = 1 + 2 / 1;
			y = 200 % (3 + 1);
			z = 30 % 3 + 2 * 4;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn shift() {
		let src: &str = "
			int x;
			x = 10 << 2 + 3 % 2 >> 3;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn bitops() {
		let src: &str = "
			2 + (3 + 5) * 6;
			1 ^ 2 | 2 != 3 / 2;
			1 + -1 ^ 2;
			3 ^ 2 & 1 | 2 & 9;
			x = 10;
			y = &x;
			3 ^ 2 & *y | 2 & &x;
			z = ~x;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn logops() {
		let src: &str = "
			1 && 2 || 3 && 4;
			1 && 2 ^ 3 || 4 && 5 || 6;
			!2;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn inc_dec() {
		let src: &str = "
			int i;
			i = 0;
			++i;
			--i;
			i++;
			i--;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}


	#[test]
	fn for_() {
		let src: &str = "
			int sum, i;
			sum = 10;
			sum = sum + i;
			for (i = 1 ; i < 10; i = i + 1) sum = sum +i;
			return sum;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn while_() {
		let src: &str = "
			int sum;
			sum = 10;
			while(sum > 0) sum = sum - 1;
			return sum;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn if_() {
		let src: &str = "
			int i;
			i = 10;
			if (i == 10) i = i / 5;
			if (i == 2) i = i + 5; else i = i / 5;
			return i;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}


	#[test]
	fn ctrls() {
		let src: &str = "
			int i, sum;
			sum = 0;
			i = 10;
			if (i == 10) while(i < 0) for(;;) sum = sum + 1;
			return sum;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn block() {
		let src: &str = "
			for( i = 10; ; ) {i = i + 1;}
			{}
			{i = i + 1; 10;}
			return 10;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn block2() {
		let src: &str = "
			int i, x;
			while(i < 10) {i = i + 1; i = i * 2;}
			x = 10;
			if ( x == 10 ){
				x = x + 200;
				x = x / 20;
			} else {
				x = x - 20;
				;
			}
			{{}}
			{i = i + 1; 10;}
			return 200;
			return;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn func() {
		let src: &str = "
			int i, j;
			call_fprint();
			i = getOne();
			j = getTwo();
			return i + j;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn func2() {
		let src: &str = "
			int i, j, k;
			call_fprint();
			i = get(1);
			j = get(2, 3, 4);
			k = get(i+j, (i=3), k);
			return i + j;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn addr_deref() {
		let src: &str = "
			int x, y, z;
			x = 3;
			y = 5;
			z = &y + 8;
			return *z;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn addr_deref2() {
		let src: &str = "
			int x;
			int *y;
			int **z;
			x = 3;
			y = &x;
			z = &y;
			return *&**z;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn comma() {
		let src: &str = "
			int x, y, z;
			x = 3, y = 4, z = 10;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn assign_op() {
		let src: &str = "
			int x;
			x = 10;
			x += 1;
			x <<= 1;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("stmt{} {}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		} 
	}

	#[test]
	fn declare() {
		let src: &str = "
			func(int x, int y) {
				return x + y;
			}
			calc(int a, int b, int c, int d, int e, int f) {
				return a*b + c - d + e/f;
			}
			main() {
				int i, sum;
				i = 0;
				sum = 0;
				for (; i < 10; i=i+1) {
					sum = sum + i;
				}
				return func(i, sum);
			}
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("declare{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	#[test]
	fn no_return() {
		let src: &str = "
			int func(int x, int y) {
				return x + y;
			}
			int main() {
				int i, sum, x, y, z;
				i = 0;
				sum = 0;
				for (; i < 10; i=i+1) {
					sum = sum + i;
				}
				func(x=1, (y=1, z=1));
			}
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("declare{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}

	// wip() を「サポートしている構文を全て使用したテスト」と定めることにする
	#[test]
	fn wip() {
		let src: &str = "
			int func(int x, int y) {
				print_helper(x+y);
				return x + y;
			}
			
			int fib(int N) {
				if (N <= 2) return 1;
				return fib(N-1) + fib(N-2);
			}
			
			int main() {
				int i; i = 0;
				int j; j = 0;
				int k; k = 1;
				int sum; sum = 0;
				for (; i < 10; i+=i+1, j++) {
					sum++;
				}
				print_helper(j);
				print_helper(k);
				while (j > 0, 0) {
					j /= 2;
					k <<= 1;
				}
				if (1 && !(k/2)) k--;
				else k = -1;
			
				int x, y, z;
				func(x=1, (y=1, z=~1));
			
				x = 15 & 10;
				x = (++x) + y;
				int *p; p = &x; 
				int **pp; pp = &p;
				print_helper(z = fib(*&(**pp)));
				func(getOne(), getTwo());
				*x = 10;
			}
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = program(&mut token_ptr);
		let mut count: usize = 1;
		for node_ptr in node_heads {
			println!("declare{}{}", count, ">".to_string().repeat(REP));
			search_tree(&node_ptr);
			count += 1;
		}
	}
}