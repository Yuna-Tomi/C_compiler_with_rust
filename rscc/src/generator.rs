use std::sync::Mutex;

use once_cell::sync::Lazy;

use crate::{
	asm_write, error_with_node, exit_eprintln, lea, mov, mov_to, mov_from, operate,
	asm::reg_ax,
	node::{Nodekind, NodeRef},
	typecell::Type
};

pub static ASM: Lazy<Mutex<String>> = Lazy::new(
	|| Mutex::new(
		".intel_syntax noprefix\n.globl main\n".to_string()
	)
);

static CTR_COUNT: Lazy<Mutex<u32>> = Lazy::new(
	|| Mutex::new(0)
);

static ARGS_REGISTERS: Lazy<Mutex<Vec<&str>>> = Lazy::new(|| Mutex::new(vec!["rdi", "rsi", "rdx", "rcx", "r8", "r9"]));



// CTR_COUNT にアクセスして分岐ラベルのための値を得つつインクリメントする
fn get_count() -> u32 {
	let mut count = CTR_COUNT.try_lock().unwrap();
	*count += 1;
	*count
}

pub fn gen_expr(node: &NodeRef) {
	match (**node).borrow().kind {
		Nodekind::FuncDecNd => {
			{
				asm_write!("{}:\n", (**node).borrow().name.as_ref().unwrap());
			
				// プロローグ(変数の格納領域の確保)
				operate!("push", "rbp");
				mov!("rbp", "rsp");
				let pull = (**node).borrow().max_offset.unwrap();
				if pull > 0 {
					operate!("sub", "rsp", pull);
				}

				// 受け取った引数の挿入: 現在は6つの引数までなのでレジスタから値を持ってくる
				if (**node).borrow().args.len() > 6 {exit_eprintln!("現在7つ以上の引数はサポートされていません。");}
				for (ix, arg) in (&(**node).borrow().args).iter().enumerate() {
					mov!("rax", "rbp");
					operate!("sub", "rax", (*(*arg.as_ref().unwrap())).borrow().offset.as_ref().unwrap());
					let size = arg.as_ref().unwrap().borrow().typ.as_ref().unwrap().bytes();
					mov_to!(size, "rax", ARGS_REGISTERS.try_lock().unwrap()[ix]);
				}
			}
			
			// 関数内の文の処理
			let s = (**node).borrow().stmts.as_ref().unwrap().len();
			for (ix, stmt_) in (**node).borrow().stmts.as_ref().unwrap().iter().enumerate() {
				gen_expr(stmt_);
				if ix != s - 1 { operate!("pop", "rax"); }
			}

			// 上の stmts の処理で return が書かれることになっているので、エピローグなどはここに書く必要はない
			return;
		}
		Nodekind::NumNd => {
			operate!("push", (**node).borrow().val.unwrap());
			return;
		}
		Nodekind::LogAndNd => {
			let c = get_count();
			let f_anchor: String = format!(".LLogic.False{}", c);
			let e_anchor: String = format!(".LLogic.End{}", c);

			// && の左側 (short circuit であることに注意)
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0);
			operate!("je", f_anchor); // 0 なら false ゆえ残りの式の評価はせずに飛ぶ 

			// && の右側
			gen_expr((**node).borrow().right.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0);
			operate!("je", f_anchor);

			// true の場合、 rax に 1 をセットして end
			mov!("rax", 1);
			operate!("jmp", e_anchor);

			asm_write!("{}:\n", f_anchor);
			mov!("rax", 0);

			asm_write!("{}:\n", e_anchor);
			// operate!("cdqe"); // rax でなく eax を使う場合は、上位の bit をクリアする必要がある(0 をきちんと false にするため)
			operate!("push", "rax");

			return;
		}
		Nodekind::LogOrNd => {
			let c = get_count();
			let t_anchor: String = format!(".LLogic.False{}", c);
			let e_anchor: String = format!(".LLogic.End{}", c);

			// && の左側 (short circuit であることに注意)
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0);
			operate!("jne", t_anchor); // 0 なら false ゆえ残りの式の評価はせずに飛ぶ 

			// && の右側
			gen_expr((**node).borrow().right.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0);
			operate!("jne", t_anchor); 

			// false の場合、 rax に 0 をセットして end
			mov!("rax", 1);
			operate!("jmp", e_anchor);

			asm_write!("{}:\n", t_anchor);
			mov!("rax", 1);

			asm_write!("{}:\n", e_anchor);
			// operate!("cdqe"); // rax でなく eax を使う場合は、上位の bit をクリアする必要がある(0 をきちんと false にするため)
			operate!("push", "rax");

			return;
		}
		Nodekind::LogNotNd => {
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");

			// rax が 0 なら 1, そうでないなら 0 にすれば良い
			operate!("cmp", "rax", 0);
			operate!("sete", "al");
			operate!("movzb", "rax", "al");
			operate!("push", "rax");

			return;
		}
		Nodekind::BitNotNd => {
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("not", "rax");
			operate!("push", "rax");

			return;
		}
		Nodekind::LvarNd => {
			// 葉、かつローカル変数なので、あらかじめ代入した値へのアクセスを行う
			// 配列のみ、それ単体でアドレスとして解釈されるため gen_addr の結果をそのまま使うことにしてスルー
			let is_tmp = node.borrow().typ.is_none();
			let typ = node.borrow().typ.clone();
			if is_tmp || typ.clone().unwrap().typ != Type::Array {
				let bytes = typ.unwrap().bytes();
				let offset = node.borrow().offset.unwrap();
				let ax = reg_ax(bytes);
				
				mov_from!(bytes, ax, "rbp", offset);
				if bytes != 8 { operate!("cdqe"); } // rax で push するために、 eax ならば符号拡張が必要
				operate!("push", "rax");
			} else {
				gen_addr(node);
			}

			return;
		}
		Nodekind::DerefNd => {
			// gen_expr内で *expr の expr のアドレスをスタックにプッシュしたことになる
			// 配列との整合をとるために *& の場合に打ち消す必要がある
			let left = (*node).borrow().left.clone().unwrap();
			if left.borrow().kind == Nodekind::AddrNd {
				gen_addr(left.borrow().left.as_ref().unwrap());
			} else {
				gen_expr((**node).borrow().left.as_ref().unwrap());
				operate!("pop", "rax");
				mov_from!(8, "rax", "rax");
				operate!("push", "rax");
			}
			return;
		}
		Nodekind::AddrNd => {
			// gen_addr内で対応する変数のアドレスをスタックにプッシュしているので、そのままでOK
			gen_addr((**node).borrow().left.as_ref().unwrap());
			return;
		}
		Nodekind::FuncNd => {
			// 引数をレジスタに格納する処理
			push_args(&(**node).borrow().args);
			
			mov!("rax", "rsp");
			operate!("and", "rsp", "~0x0f"); // 16の倍数に align
			operate!("sub", "rsp", 8);
			operate!("push", "rax");

			// この時点で ARGS_REGISTERS に記載の6つのレジスタには引数が入っている必要がある
			operate!("call", (**node).borrow().name.as_ref().unwrap());
			operate!("pop", "rsp");
			operate!("push", "rax");
			return;
		}
		Nodekind::AssignNd => {
			// 節点、かつアサインゆえ左は左辺値の葉を想定(違えばgen_addr内でエラー)
			gen_addr((**node).borrow().left.as_ref().unwrap());
			gen_expr((**node).borrow().right.as_ref().unwrap());

			// 上記gen_expr2つでスタックに変数の値を格納すべきアドレスと、代入する値(式の評価値)がこの順で積んであるはずなので2回popして代入する
			operate!("pop", "rdi");
			operate!("pop", "rax");
			mov_to!(8, "rax", "rdi");
			operate!("push", "rdi"); // 連続代入可能なように、評価値として代入した値をpushする
			return;
		}
		Nodekind::CommaNd => {
			// 式の評価値として1つ目の結果は捨てる
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");

			// 2つ目の式の評価値はそのまま使うので、popなしでOK
			gen_expr((**node).borrow().right.as_ref().unwrap());
			return;
		}
		Nodekind::ReturnNd => {
			// リターンならleftの値を評価してretする。
			gen_expr((**node).borrow().left.as_ref().unwrap());
			operate!("pop", "rax");
			mov!("rsp", "rbp");
			operate!("pop", "rbp");
			operate!("ret");
			return;
		}
		Nodekind::IfNd => {
			let c: u32 = get_count();
			let end: String = format!(".LEnd{}", c);

			// 条件文の処理
			gen_expr((**node).borrow().enter.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0);

			// elseがある場合は微妙にjmp命令の位置が異なることに注意
			if let Some(ptr) = (**node).borrow().els.as_ref() {
				let els: String = format!(".LElse{}", c);

				// falseは0なので、cmp rax, 0が真ならelseに飛ぶ
				operate!("je", els);
				gen_expr((**node).borrow().branch.as_ref().unwrap()); // if(true)の場合の処理
				operate!("jmp", end); // elseを飛ばしてendへ

				// elseの後ろの処理
				asm_write!("{}:\n", els);
				gen_expr(ptr);
				operate!("pop", "rax"); // 今のコードでは各stmtはpush raxを最後にすることになっているので、popが必要

			} else {
				// elseがない場合の処理
				operate!("je", end);
				gen_expr((**node).borrow().branch.as_ref().unwrap());
				operate!("pop", "rax"); // 今のコードでは各stmtはpush raxを最後にすることになっているので、popが必要
			}

			// stmtでgen_exprした後にはpopが呼ばれるはずであり、分岐後いきなりpopから始まるのはおかしい(し、そのpopは使われない)
			// ブロック文やwhile文も単なる num; などと同じようにstmt自体が(使われない)戻り値を持つものだと思えば良い
			asm_write!("{}:\n", end);
			operate!("push", 0);

			return;
		}
		Nodekind::WhileNd => {
			let c: u32 = get_count();
			let begin: String = format!(".LBegin{}", c);
			let end: String = format!(".LEnd{}", c);

			asm_write!("{}:\n", begin);

			gen_expr((**node).borrow().enter.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0); // falseは0なので、cmp rax, 0が真ならエンドに飛ぶ
			operate!("je", end);

			gen_expr((**node).borrow().branch.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("jmp", begin);

			// if 文同様に push が必要
			asm_write!("{}:\n", end);
			operate!("push", 0);

			return;
		}
		Nodekind::ForNd => {
			let c: u32 = get_count();
			let begin: String = format!(".LBegin{}", c);
			let end: String = format!(".LEnd{}", c);

			if let Some(ptr) = (**node).borrow().init.as_ref() {
				gen_expr(ptr);
			}

			asm_write!("{}:\n", begin);

			gen_expr((**node).borrow().enter.as_ref().unwrap());
			operate!("pop", "rax");
			operate!("cmp", "rax", 0); // falseは0なので、cmp rax, 0が真ならエンドに飛ぶ
			operate!("je", end);
			
			gen_expr((**node).borrow().branch.as_ref().unwrap()); // for文内の処理
			operate!("pop", "rax"); // 今のコードでは各stmtはpush raxを最後にすることになっているので、popが必要
			
			gen_expr((**node).borrow().routine.as_ref().unwrap()); // インクリメントなどの処理
			operate!("jmp", begin);

			// if文と同じ理由でpushが必要
			asm_write!("{}:\n", end);
			operate!("push", 0);

			return;
		} 
		Nodekind::BlockNd => {
			for child in &(**node).borrow().children {
				// parserのコード的にNoneなchildはありえないはずであるため、直にunwrapする
				gen_expr(child.as_ref().unwrap());
				operate!("pop", "rax"); // 今のコードでは各stmtはpush raxを最後にすることになっているので、popが必要
			}
			
			// このBlock自体がstmt扱いであり、このgen_exprがreturnした先でもpop raxが生成されるはず
			// これもif文と同じくpush 0をしておく
			operate!("push", 0);

			return;
		}
		_ => {}// 他のパターンなら、ここでは何もしない
	} 

	let left = (*node).borrow().left.clone().unwrap();
	let right = (*node).borrow().right.clone().unwrap();
	gen_expr(&left);
	gen_expr(&right);

	// long や long long などが実装されるまではポインタなら8バイト、そうでなければ4バイトのレジスタを使うことにする
	let (ax, di, dx, cq) = if left.borrow().typ.as_ref().unwrap().ptr_end.is_some() {
		("rax", "rdi", "rdx", "cqo") 
	} else {
		("eax", "edi", "edx", "cdq") 
	};


	if [Nodekind::LShiftNd, Nodekind::RShiftNd].contains(&(**node).borrow().kind) {
		operate!("pop", "rcx");
	} else {
		operate!("pop", "rdi");
	}
	operate!("pop", "rax");

	// >, >= についてはオペランド入れ替えのもとsetl, setleを使う
	match (**node).borrow().kind {
		Nodekind::AddNd => {
			operate!("add", ax, di);
		}
		Nodekind::SubNd => {
			operate!("sub", ax, di);
		}
		Nodekind::MulNd => {
			operate!("imul", ax, di);
		}
		Nodekind::DivNd  => {
			operate!(cq); // rax -> rdx:rax に拡張(ただの 0 fill)
			operate!("idiv", di); // rdi で割る: rax が商で rdx が剰余になる
		}
		Nodekind::ModNd  => {
			operate!(cq);
			operate!("idiv", di);
			operate!("push", dx);
			return;
		}
		Nodekind::LShiftNd => {
			operate!("sal", ax, "cl");
		}
		Nodekind::RShiftNd => {
			operate!("sar", ax, "cl");
		}
		Nodekind::BitAndNd => {
			operate!("and", ax, di);
		}
		Nodekind::BitOrNd => {
			operate!("or", ax, di);
		}
		Nodekind::BitXorNd => {
			operate!("xor", ax, di);
		}
		Nodekind::EqNd => {
			operate!("cmp", ax, di);
			operate!("sete", "al");
			operate!("movzb", "rax", "al");
		}
		Nodekind::NEqNd => {
			operate!("cmp", ax, di);
			operate!("setne", "al");
			operate!("movzb", "rax", "al");
		}
		Nodekind::LThanNd => {
			operate!("cmp", ax, di);
			operate!("setl", "al");
			operate!("movzb", "rax", "al");
		}
		Nodekind::LEqNd => {
			operate!("cmp", ax, di);
			operate!("setle", "al");
			operate!("movzb", "rax", "al");
		}
		_ => {
			// 上記にないNodekindはここに到達する前にreturnしているはず
			error_with_node!("不正な Nodekind です。", &*(**node).borrow());
		}
	}

	operate!("push", "rax");
}

// アドレスを生成する関数(ポインタでない普通の変数への代入等でも使用)
fn gen_addr(node: &NodeRef) {
	match (**node).borrow().kind {
		Nodekind::LvarNd => {
			// 変数に対応するアドレスをスタックにプッシュする
			let offset = node.borrow().offset.unwrap();
			lea!("rax", "rbp", offset);
			operate!("push", "rax");
		}
		Nodekind::DerefNd => {
			// *expr: exprで計算されたアドレスを返したいので直で gen_expr する(例えば&*のような書き方だと打ち消される)
			gen_expr((**node).borrow().left.as_ref().unwrap());
		}
		_ => {
			error_with_node!("左辺値が変数ではありません。", &*(**node).borrow());
		}
	}
}

// 関数呼び出し時の引数の処理を行う関数
fn push_args(args: &Vec<Option<NodeRef>>) {
	let argc =  args.len();
	if argc > 6 {exit_eprintln!("現在7つ以上の引数はサポートされていません。");}

	// 計算時に rdi などを使う場合があるので、引数はまずはスタックに全て push したままにしておく
	// おそらく、逆順にしておいた方がスタックに引数を積みたくなった場合に都合が良い
	for i in (0..argc).rev() {
		gen_expr(&(args[i]).as_ref().unwrap());
	}

	for i in 0..argc {
		operate!("pop", (*ARGS_REGISTERS.try_lock().unwrap())[i]);
	}
}

#[cfg(test)]
mod tests {
	use crate::parser::{
		expr, program,
		tests::parse_stmts,
	};
	use crate::tokenizer::tokenize;
	use crate::globals::{CODES, FILE_NAMES};
	use super::*;

	fn test_init(src: &str) {
		let mut src_: Vec<String> = src.split("\n").map(|s| s.to_string()+"\n").collect();
		FILE_NAMES.try_lock().unwrap().push("test".to_string());
		let mut code = vec!["".to_string()];
		code.append(&mut src_);
		CODES.try_lock().unwrap().push(code);
	}

	#[test]
	fn addsub() {
		let src: &str = "
			1+2+3-1
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn muldiv() {
		let src: &str = "
			1+2*3-4/2+3%2
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn brackets() {
		let src: &str = "
			(1+2)/3-1*20
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn unary() {
		let src: &str = "
			(-1+2)*(-1)+(+3)/(+1)
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn shift() {
		let src: &str = "
			200 % 3 << 4 + 8 >> 8
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}
	
	#[test]
	fn eq() {
		let src: &str = "
			(-1+2)*(-1)+(+3)/(+1) == 30 + 1
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_ptr = expr(&mut token_ptr);
		gen_expr(&node_ptr);
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn assign1() {
		let src: &str = "
			int a;
			a = 1; a + 1;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn assign2() {
		let src: &str = "
			int local, local_value, local_value99;
			local = 1; local_value = local + 1; local_value99 = local_value + 3;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn bitops() {
		let src: &str = "
			int x, y;
			2 + (3 + 5) * 6;
			1 ^ 2 | 2 != 3 / 2;
			1 + -1 ^ 2;
			3 ^ 2 & 1 | 2 & 9;
			x = 10;
			y = &x;
			3 ^ 2 & *y | 2 & &x;
			~x ^ ~*y | 2;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn logops() {
		let src: &str = "
			int x, y, z, q;
			x = 10;
			y = 20;
			z = 20;
			q = !x && !!y - z || 0;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn comma() {
		let src: &str = "
			int x, y, z;
			x = 10, y = 10, z = 10;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn if_() {
		let src: &str = "
			int i;
			i = 10;
			if (1) i + 1;
			x = i + 10;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn while_() {
		let src: &str = "
			int i;
			i = 10;
			while (i > 1) i = i - 1;
			i;
		";
		test_init(src);
		
		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn for_() {
		let src: &str = "
			int sum, i;
			sum = 10;
			for (i = 0; i < 10; i = i + 1) sum = sum + i;
			return sum;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}
	
	#[test]
	fn block() {
		let src: &str = "
			int sum, sum2, i;
			sum = 10;
			sum2 = 20;
			for (i = 0; i < 10; i = i + 1) {
				sum = sum + i;
				sum2 = sum2 + i;
			}
			return sum;
			return;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}
	
	#[test]
	fn func() {
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
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
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
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn addr_deref2() {
		let src: &str = "
			int x, y, z;
			x = 3;
			y = &x;
			z = &y;
			return *&**z;
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = parse_stmts(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
			*ASM.try_lock().unwrap() += "	pop rax\n";
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn funcdec() {
		let src: &str = "
			int func(int x, int y) {
				return x * (y + 1);
			}
			int sum(int i, int j) {
				return i + j;
			}
			int main() {
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
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
		}
		println!("{}", ASM.try_lock().unwrap());
	}

	#[test]
	fn recursion() {
		let src: &str = "
			int fib(int n) {
				return fib(n-1)+fib(n-2);
			}
			int main() {
				return fib(10);
			}
		";
		test_init(src);

		let mut token_ptr = tokenize(0);
		let node_heads = program(&mut token_ptr);
		for node_ptr in node_heads {
			gen_expr(&node_ptr);
		}
		println!("{}", ASM.try_lock().unwrap());
	}
}