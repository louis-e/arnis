$function arnis:walk/pick {list:"$(list)"}
$data modify storage arnis:walk spawn.eq.$(slot) set value {id:"minecraft:$(item)",count:1,components:{"minecraft:dyed_color":0}}
$data modify storage arnis:walk spawn.eq.$(slot).components."minecraft:dyed_color" set from storage arnis:walk pick.v
