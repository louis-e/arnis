function arnis:walk/pick {list:"bags"}
data modify storage arnis:walk spawn.eq.mainhand set value {count:1}
data modify storage arnis:walk spawn.eq.mainhand.id set from storage arnis:walk pick.v
